//! Object script interpreter and opcode implementations (docs/re/objects.md §2,
//! docs/re/object_opcodes.txt). Coordinates are map pixels; the cartridge's ROM-space constants
//! (map + 0x80) are converted where they appear.
use super::{
    super::{
        anim::SpriteAnimator,
        collision::{self, Collider, Contacts},
        coords::{Velocity, WorldPos, px},
        data::{InitPreset, Side, SpawnRecord, SpriteSheet},
        player::{Player, PlayerState},
    },
    Obj, Pool, RoomCtx, SpawnQueue, SpawnRequest, anim_key, combat::HitBox, flag,
};
use bevy::prelude::*;
use super::super::audio::MUSIC_TIMER;

/// Boss fight in progress (cartridge word 0xFF0B92): room exits are disabled while it is set (0x41AA).
pub const BOSS_FLAG: u32 = 0xFF0B92;
use std::collections::HashSet;

pub(crate) fn fp(v: f64) -> i32 {
    (v * 65536.0).round() as i32
}

/// What the scripts can see of a player.
#[derive(Debug, Clone, Copy)]
pub struct PlayerView {
    pub x: i32,
    pub y: i32,
    pub alive: bool,
    pub hurt: Option<HitBox>,
    pub attack: Option<HitBox>,
    /// Swinging (cartridge player +124 attack power != 0).
    pub attacking: bool,
    /// Player's swing id (changes every new swing).
    pub swing: u32,
    pub facing_right: bool,
}

#[derive(Default, Debug, Clone)]
pub struct PlayersView {
    pub players: Vec<PlayerView>,
}

pub type PlayerQueryData = (&'static Player, &'static WorldPos, &'static SpriteAnimator);

impl PlayersView {
    pub fn collect(q: &Query<PlayerQueryData, Without<Obj>>, sheets: &Assets<SpriteSheet>) -> Self {
        let mut players = vec![];
        // Indexed by player slot (opcodes 102/103 address player 1 / player 2).
        let mut sorted: Vec<_> = q.iter().collect();
        sorted.sort_by_key(|(p, ..)| p.slot);
        for (p, pos, anim) in sorted {
            let (hurt, attack) = sheets
                .get(&anim.sheet)
                .map(|s| super::combat::player_boxes(s, anim, pos))
                .unwrap_or((None, None));
            players.push(PlayerView {
                x: pos.px_x(),
                y: pos.px_y(),
                alive: p.state < PlayerState::Dying,
                hurt,
                attack: if p.attack_power() > 0 { attack } else { None },
                attacking: p.attack_power() > 0,
                swing: p.swing_id,
                facing_right: p.facing == super::super::player::Facing::Right,
            });
        }
        Self { players }
    }
    pub fn nearest(&self, x: i32) -> Option<&PlayerView> {
        self.players.iter().filter(|p| p.alive).min_by_key(|p| (p.x - x).abs())
    }
    /// Proximity test 0x1C29E: `p - d < obj <= p + d` on each axis (asymmetric: an object exactly
    /// `d` to the left of the player is not near); a zero range skips that axis.
    pub(crate) fn near(&self, x: i32, y: i32, dx: i32, dy: Option<i32>) -> bool {
        let within = |o: i32, p: i32, d: i32| d == 0 || (o - p > -d && o - p <= d);
        self.players.iter().any(|p| p.alive && within(x, p.x, dx) && dy.is_none_or(|dy| within(y, p.y, dy)))
    }
}

/// Snapshot of another object for cross-object tests.
#[derive(Debug, Clone, Copy)]
pub struct OtherObj {
    pub slot: usize,
    pub ty: u16,
    pub x: i32,
    pub y: i32,
    pub state: u16,
    pub active: bool,
    pub hp: i32,
    pub hit_count: u32,
    pub flash: u32,
    pub vars: [i32; super::VARS],
    /// Exact 16.16 position (cartridge long x/y), for objects that copy another's position.
    pub wx: i32,
    pub wy: i32,
    /// (dx, dy) of the first piece of the current mapping (cartridge `mapping+18`), see bosses.rs.
    pub piece0: (i32, i32),
}

impl OtherObj {
    pub fn var(&self, offset: usize) -> i32 {
        self.vars.get((offset - 158) / 2).copied().unwrap_or(0)
    }
}

/// Copies the current frame's mapping into the object's variables at `base` (+0xAC: 41 words,
/// +0x9E: 49 words) so scripts can move individual pieces (cartridge I152/I158).
pub fn copy_mapping(ctx: &RoomCtx, o: &mut Obj, anim: &SpriteAnimator, base: usize) {
    let Some(def) = ctx.sheet.anims.get(&anim.anim) else { return };
    let Some(af) = def.frames.get(anim.index) else { return };
    let frame = &ctx.sheet.frames[af.frame];
    let Some(fp) = ctx.sheet.frame_pieces.get(&af.frame.to_string()) else { return };
    o.ram_map = Some(base);
    o.set_var(base, fp.count as i32);
    for (i, b) in frame.boxes.iter().enumerate() {
        o.set_var(base + 2 + 2 * i, *b as i32);
    }
    for (k, p) in fp.pieces.iter().enumerate() {
        let off = base + 18 + 8 * k;
        if off + 6 > 158 + 2 * (super::VARS - 1) {
            break;
        }
        o.set_var(off, p[0]);
        o.set_var(off + 2, p[1]);
        o.set_var(off + 4, p[2]);
        o.set_var(off + 6, p[3]);
    }
    o.boxes = Some(frame.boxes);
}

/// Deferred change to another object (applied after all objects ticked).
#[derive(Debug, Clone, Copy)]
pub enum SlotWrite {
    Var { slot: usize, offset: usize, value: i32 },
    State { slot: usize, state: u16 },
    Delete { slot: usize },
    /// Position in 16.16 map pixels (boss scripts placing their segments).
    Pos { slot: usize, x: i32, y: i32 },
    /// flags = (flags & and) | or.
    Flags { slot: usize, and: u16, or: u16 },
}

/// Side effects requested by scripts, applied after all objects ticked.
#[derive(Resource, Default, Debug)]
pub struct Effects {
    pub spawns: Vec<SpawnRequest>,
    pub messages: Vec<u32>,
    pub persist: Vec<u16>,
    pub sounds: Vec<u32>,
    pub budget_refund: u32,
    /// Sprite palette changes for `sprite_palette` (boss hit flash, graphics-bank palette switches).
    pub palette_ops: Vec<super::super::sprite_palette::PaletteOp>,
    pub clear_slots: Option<(usize, usize)>,
    pub others: Vec<OtherObj>,
    pub camera: IVec2,
    pub unknown: HashSet<String>,
    pub room: usize,
    pub slot_writes: Vec<SlotWrite>,
    /// Global script variables (cartridge RAM words such as 0xFF0A74 event, 0xFF0B92 boss active).
    pub globals: std::collections::HashMap<u32, i32>,
    pub set_globals: Vec<(u32, i32)>,
    pub delete_all: bool,
    /// The Mage's spell as seen by object scripts this tick (C124).
    pub spells: super::super::magic::SpellView,
}

impl Effects {
    pub(crate) fn slot(&self, slot: usize) -> Option<&OtherObj> {
        self.others.iter().find(|o| o.slot == slot && o.active)
    }
    /// Makes a requested spawn visible to `slot()` right away: the cartridge creates objects
    /// immediately, so scripts may test the slot in the same tick (e.g. Kraken C134).
    pub(crate) fn predict_spawn(&mut self, req: &SpawnRequest) {
        let Some(slot) = super::slot_order(req.pool).into_iter().find(|&s| self.slot(s).is_none()) else { return };
        self.others.push(OtherObj {
            slot,
            ty: req.record.ty,
            x: req.x,
            y: req.y,
            state: 0,
            active: true,
            hp: req.record.hp as i32,
            hit_count: 0,
            flash: 0,
            vars: [0; super::VARS],
            wx: req.x << 16,
            wy: req.y << 16,
            piece0: (0, 0),
        });
    }
    pub(crate) fn global(&self, addr: u32) -> i32 {
        self.globals.get(&addr).copied().unwrap_or(0)
    }
    pub(crate) fn set_global(&mut self, addr: u32, v: i32) {
        self.globals.insert(addr, v);
        self.set_globals.push((addr, v));
    }
}

/// Direction code from (x0,y0) to (x1,y1) (cartridge 0x1C2F2): bit0 right, bit1 down,
/// +4 horizontal-dominant, +8 diagonal.
pub fn direction_code(x0: i32, y0: i32, x1: i32, y1: i32) -> u8 {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let mut d = 0u8;
    if dx >= 0 {
        d |= 1;
    }
    if dy >= 0 {
        d |= 2;
    }
    let (ax, ay) = (dx.abs(), dy.abs());
    if ax >= ay {
        d += 4;
        if (ax >> 1) - ay < 0 {
            d += 8;
        }
    } else if (ay >> 1) - ax < 0 {
        d += 8;
    }
    d
}

/// Velocity for "aim at the player" by direction code (cartridge 0x17270, table 0x172B0).
fn aim_velocity(code: u8) -> (f64, f64) {
    let (a, b, c) = (4.0, 1.75, 2.75);
    match code {
        0 | 1 => (0.0, -a),
        2 => (-b, 2.0 * b),
        3 => (b, 2.0 * b),
        4 => (-2.0 * b, -b),
        5 => (2.0 * b, -b),
        6 => (-a, 0.0),
        7 => (a, 0.0),
        8 => (-b, -2.0 * b),
        9 => (b, -2.0 * b),
        10 => (-c, c),
        11 => (c, c),
        12 => (-c, -c),
        13 => (c, -c),
        14 => (-2.0 * b, b),
        _ => (2.0 * b, b),
    }
}

fn unknown(fx: &mut Effects, op: String) {
    if fx.unknown.insert(op.clone()) {
        warn!("object opcode {op} not implemented");
    }
}

pub(crate) fn spawn(ctx: &RoomCtx, o: &Obj, pos: &WorldPos, fx: &mut Effects, n: usize, dx: i32, dy: i32, pool: Pool) {
    let Some(rec) = ctx.objs.records.get(n) else { return };
    let req = SpawnRequest { record: *rec, x: pos.px_x() + dx, y: pos.px_y() + dy, pool, marker: None, gfx: o.gfx.clone() };
    fx.predict_spawn(&req);
    fx.spawns.push(req);
}

/// Spawn at an absolute position given in cartridge ROM space (map + 0x80).
pub(crate) fn spawn_abs(ctx: &RoomCtx, o: &Obj, fx: &mut Effects, n: usize, rom_x: i32, rom_y: i32, pool: Pool) {
    let Some(rec) = ctx.objs.records.get(n) else { return };
    let req = SpawnRequest { record: *rec, x: rom_x - 0x80, y: rom_y - 0x80, pool, marker: None, gfx: o.gfx.clone() };
    fx.predict_spawn(&req);
    fx.spawns.push(req);
}

/// Switches to state `s` (cartridge 0x141AE).
#[allow(clippy::too_many_arguments)]
pub fn enter_state(
    ctx: &RoomCtx,
    o: &mut Obj,
    s: u16,
    pos: &mut WorldPos,
    vel: &mut Velocity,
    anim: &mut SpriteAnimator,
    col: &mut Collider,
    p: &PlayersView,
    fx: &mut Effects,
) {
    o.state = s;
    let Some(st) = ctx.objs.types.get(&o.ty.to_string()).and_then(|t| t.states.get(&s.to_string())).cloned() else {
        o.delete = true;
        return;
    };
    o.prev_state_time = o.state_timer.saturating_sub(2).max(1);
    o.state_timer = 0;
    if st.gfx != "default" {
        o.gfx = st.gfx.clone();
    }
    if let Some(a) = st.anim {
        // set_anim (0x269A) points the mapping back into ROM: a script-edited copy is dropped.
        o.ram_map = None;
        o.boxes = None;
        o.anim_id = Some(a);
        let key = anim_key(a, o.tile_ofs, &o.gfx);
        anim.restart(&key);
        if let Some([w, h]) = ctx.objs.anim_collision.get(&key) {
            col.half_width = (w / 2).max(1);
            col.height = (*h).max(1);
        }
    }
    init_op(ctx, o, st.init, pos, vel, anim, p, fx);
    if vel.x != 0 {
        o.face_left = vel.x < 0;
    }
}

/// Runs the behaviour and conditions of the current state; a satisfied condition switches state
/// and re-runs the new state in the same tick (cartridge 0x14BA0).
#[allow(clippy::too_many_arguments)]
pub fn tick(
    ctx: &RoomCtx,
    o: &mut Obj,
    pos: &mut WorldPos,
    vel: &mut Velocity,
    anim: &mut SpriteAnimator,
    col: &mut Collider,
    c: &mut Contacts,
    p: &PlayersView,
    fx: &mut Effects,
) {
    for _ in 0..8 {
        let Some(st) = ctx.objs.types.get(&o.ty.to_string()).and_then(|t| t.states.get(&o.state.to_string())).cloned() else {
            o.delete = true;
            return;
        };
        behaviour(ctx, o, st.behaviour, pos, vel, anim, col, c, p, fx);
        if o.delete {
            return;
        }
        let mut next = None;
        for [cond, goto] in &st.conds {
            if condition(ctx, o, *cond, pos, vel, anim, c, p, fx) {
                next = Some(*goto);
                break;
            }
            if o.delete {
                return;
            }
        }
        if o.pending_death {
            become_death_type(ctx, o, pos, vel, anim, col, p, fx);
            return;
        }
        match next {
            Some(n) => enter_state(ctx, o, n, pos, vel, anim, col, p, fx),
            None => return,
        }
    }
}

fn move_collide(ctx: &RoomCtx, col: &Collider, pos: &mut WorldPos, vel: &mut Velocity, c: &mut Contacts) {
    collision::step(ctx.def, ctx.data, col, pos, vel, c);
}

#[allow(clippy::too_many_arguments)]
fn behaviour(
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
) {
    match op {
        0 => {}
        1 => move_collide(ctx, col, pos, vel, c),
        2 | 18 => {
            vel.y = (vel.y + fp(0.125)).min(fp(6.0));
            move_collide(ctx, col, pos, vel, c);
        }
        3 | 19 => {
            vel.y += o.ay;
            pos.x += vel.x;
            pos.y += vel.y;
        }
        4 | 10..=15 | 20..=31 => {
            vel.y = fp(4.0);
            move_collide(ctx, col, pos, vel, c);
        }
        5 => {
            vel.y += o.ay;
            vel.x += o.ax;
            move_collide(ctx, col, pos, vel, c);
        }
        6 => {
            vel.y += o.ay;
            vel.x += o.ax;
            pos.x += vel.x;
            pos.y += vel.y;
        }
        7 => {
            vel.x += o.ax;
            collision::horizontal_only(ctx.def, col, pos, vel, c);
        }
        8 => o.delete = true,
        16 | 17 => {
            if let Some(pl) = p.players.get((op - 16) as usize) {
                *pos = WorldPos::from_px(pl.x, pl.y);
            }
        }
        32 => {
            fx.sounds.push(0xB0);
            for n in [5, 6, 7] {
                spawn(ctx, o, pos, fx, n, 0, 0, Pool::High);
            }
        }
        33 => spawn(ctx, o, pos, fx, 12, 0, 0, Pool::High),
        34 => spawn(ctx, o, pos, fx, 13, 0, 0, Pool::High),
        35 => turn_into_drop(ctx, o, pos, vel, anim, col, p, fx, false),
        62 => turn_into_drop(ctx, o, pos, vel, anim, col, p, fx, true),
        63 => {
            vel.y += o.ay;
            vel.x += o.ax;
            move_collide(ctx, col, pos, vel, c);
            let same_sign = (vel.x >= 0) == (vel.y >= 0);
            let a = o.var(190) as u32 + if same_sign { 0x14 } else { 0x15 };
            if o.anim_id != Some(a) {
                o.anim_id = Some(a);
                anim.restart(&anim_key(a, o.tile_ofs, &o.gfx));
            }
        }
        64 => spawn(ctx, o, pos, fx, 1, 0, -16, Pool::High),
        65 => {
            fx.persist.push(o.param118);
            fx.clear_slots = Some((1, 10));
            for n in 3..=10 {
                spawn(ctx, o, pos, fx, n, 0, -32, Pool::Any);
            }
            for dx in [-24, -8, 8, 24] {
                spawn(ctx, o, pos, fx, 2, dx, 0, Pool::Any);
            }
        }
        96 => o.flags &= !flag::SOLID,
        9 => {
            vel.y += o.ay;
            vel.x += o.ax;
            vel.x = vel.x.clamp(fp(-3.0), fp(3.0));
            vel.y = vel.y.clamp(fp(-2.0), fp(2.0));
            pos.x += vel.x;
            pos.y += vel.y;
        }
        36 => spawn(ctx, o, pos, fx, 1, 0, -32, Pool::High),
        38 => spawn(ctx, o, pos, fx, 3, -24, -44, Pool::High),
        39 => spawn(ctx, o, pos, fx, 4, 24, -44, Pool::High),
        40 => {
            if o.state_timer % 16 == 0 && o.state_timer > 0 {
                vel.y = -vel.y;
            }
            vel.y += o.ay;
            vel.x += o.ax;
            pos.x += vel.x;
            pos.y += vel.y;
        }
        41 => {
            let home = o.var(158);
            vel.y += o.ay;
            pos.y += vel.y;
            let y = pos.px_y();
            if y < home - 48 {
                o.ay = fp(0.125);
            } else if y > home - 16 {
                o.ay = fp(-0.125);
            }
            vel.y = vel.y.clamp(fp(-1.9375), fp(1.9375));
        }
        42 => spawn(ctx, o, pos, fx, 7, 0, -8, Pool::High),
        43 => spawn(ctx, o, pos, fx, 8, 0, -8, Pool::High),
        44 => {
            for n in [3, 4, 5] {
                spawn(ctx, o, pos, fx, n, 0, -16, Pool::High);
            }
        }
        45 => {
            if let Some(n) = p.nearest(pos.px_x()) {
                o.ax = if n.x > pos.px_x() { fp(0.0625) } else { fp(-0.0625) };
                o.ay = if n.y - 32 > pos.px_y() { fp(0.0625) } else { fp(-0.0625) };
            }
            vel.x = (vel.x + o.ax).clamp(fp(-2.0), fp(2.0));
            vel.y = (vel.y + o.ay).clamp(fp(-2.0), fp(2.0));
            pos.x += vel.x;
            pos.y += vel.y;
        }
        46 => {
            for n in [16, 17, 18] {
                spawn(ctx, o, pos, fx, n, 0, -16, Pool::High);
            }
        }
        49 => spawn(ctx, o, pos, fx, 9, -40, -16, Pool::High),
        50 => spawn(ctx, o, pos, fx, 10, 40, -16, Pool::High),
        53 => spawn(ctx, o, pos, fx, 14, 0, 0, Pool::High),
        54 => spawn(ctx, o, pos, fx, 15, 0, 0, Pool::High),
        55 => {
            if let Some(n) = p.nearest(pos.px_x()) {
                pos.x += px(32 * (n.x - pos.px_x()).signum());
                pos.y += px(32 * (n.y - pos.px_y()).signum());
            }
        }
        57 => {
            spawn(ctx, o, pos, fx, 1, 32, 0, Pool::High);
            spawn(ctx, o, pos, fx, 1, -32, 0, Pool::High);
        }
        58 => {
            for n in 20..=24 {
                spawn(ctx, o, pos, fx, n, 0, 0, Pool::High);
            }
        }
        92 => {
            spawn(ctx, o, pos, fx, 29, -48, 0, Pool::Any);
            fx.persist.push(o.param118);
        }
        93 => {
            if o.state_timer == 0 {
                fx.messages.push(0x30);
            }
            vel.y = fp(4.0);
            move_collide(ctx, col, pos, vel, c);
        }
        94 | 95 | 103 | 104 | 115 | 116 | 128 | 129 => {
            if o.state_timer == 0 {
                let msg = match op {
                    94 => 0x18,
                    95 => 0x19,
                    103 => 0x3E,
                    104 => 0x5A,
                    115 => 0x42,
                    116 => 0x43,
                    128 => 0x3D,
                    _ => 0x44,
                };
                fx.messages.push(msg);
                if op == 95 {
                    spawn_abs(ctx, o, fx, 6, 544, 128, Pool::High);
                }
            }
            if op == 128 && o.state_timer == 2 {
                fx.set_global(0xFF0A74, 2);
            }
        }
        97 => spawn(ctx, o, pos, fx, 26, 0, -16, Pool::High),
        99 => {
            fx.set_global(0xFF0A5A, 0x100);
            o.delete = true;
        }
        105 => fx.set_global(0xFF0A74, 1),
        110 => {
            if o.state_timer % 16 == 0 && o.state_timer > 0 {
                vel.x = -vel.x;
            }
            vel.y += o.ay;
            vel.x += o.ax;
            move_collide(ctx, col, pos, vel, c);
        }
        117 => fx.set_global(0xFF0A74, 3),
        134 => fx.set_global(0xFF0A74, 4),
        _ if super::bosses::behaviour(ctx, o, op, pos, vel, anim, col, c, p, fx) => {}
        _ => unknown(fx, format!("B{op}")),
    }
}

/// Becomes a pickup (gold bag or item) or disappears (B35; B62 for chests also sets the persist flag).
#[allow(clippy::too_many_arguments)]
fn turn_into_drop(
    ctx: &RoomCtx,
    o: &mut Obj,
    pos: &mut WorldPos,
    vel: &mut Velocity,
    anim: &mut SpriteAnimator,
    col: &mut Collider,
    p: &PlayersView,
    fx: &mut Effects,
    chest: bool,
) {
    if chest {
        if o.gold == 0x800C {
            o.ty = 12;
            o.flags = 0x4013;
        } else {
            o.ty = if o.gold & 0x8000 != 0 { o.gold & 15 } else { 2 };
            o.flags = 0x4003;
        }
        fx.persist.push(o.param118);
    } else {
        if o.gold == 0 {
            o.delete = true;
            return;
        }
        o.ty = if o.gold & 0x8000 != 0 { o.gold & 15 } else { 2 };
        o.flags = (o.flags & 0x80) | 0x4003;
    }
    fx.budget_refund += o.cost.saturating_sub(1) as u32;
    o.cost = 1;
    o.tile_ofs = 0;
    enter_state(ctx, o, 0, pos, vel, anim, col, p, fx);
}

fn apply_preset(o: &mut Obj, vel: &mut Velocity, pr: &InitPreset, fx: &mut Effects) {
    if let Some(v) = pr.vx {
        vel.x = fp(v);
    }
    if let Some(v) = pr.vy {
        vel.y = fp(v);
    }
    if let Some(v) = pr.ax {
        o.ax = fp(v);
    }
    if let Some(v) = pr.ay {
        o.ay = fp(v);
    }
    if let Some(f) = pr.face_left {
        o.face_left = f != 0;
    }
    if pr.despawn_timer.is_some() {
        o.despawn_timer = 1;
    }
    if let Some(s) = pr.sound {
        fx.sounds.push(s);
    }
}

#[allow(clippy::too_many_arguments)]
fn init_op(
    ctx: &RoomCtx,
    o: &mut Obj,
    op: u16,
    pos: &mut WorldPos,
    vel: &mut Velocity,
    _anim: &mut SpriteAnimator,
    _p: &PlayersView,
    fx: &mut Effects,
) {
    if let Some(pr) = ctx.ops.init.get(&op.to_string()) {
        let pr = *pr;
        apply_preset(o, vel, &pr, fx);
        // A few parsed presets have extra side effects.
        match op {
            132 | 133 | 140 | 141 => o.set_var(162, o.var(162) + 1),
            134 | 135 => o.set_var(162, 0),
            _ => {}
        }
        return;
    }
    match op {
        75 => o.flags &= !flag::CLANG,
        78 => o.flags = 8192,
        130 => fx.sounds.push(0xB6),
        131 => {
            o.set_var(162, 4);
            fx.sounds.push(0x8B);
            fx.set_global(BOSS_FLAG, 1);
            fx.messages.push(if fx.room == 3 { 0x56 } else { 0x57 });
        }
        132 | 133 => {
            vel.x = fp(if op == 132 { 1.125 } else { -1.125 });
            vel.y = fp(-4.0);
            o.ax = 0;
            o.ay = fp(0.25);
            fx.sounds.push(0xAD);
            o.set_var(162, o.var(162) + 1);
        }
        134 | 135 => {
            vel.x = fp(if op == 134 { 6.0 } else { -6.0 });
            vel.y = fp(-8.0);
            o.ax = 0;
            o.ay = fp(0.375);
            fx.sounds.push(0xAD);
            o.set_var(162, 0);
        }
        136 => {
            vel.x = -vel.x;
            vel.y = -vel.y;
        }
        137 => {
            vel.x = 0;
            vel.y = fp(-6.0);
            o.ax = 0;
            o.ay = 0;
            fx.sounds.push(0xAD);
            o.hit_count = 0;
            o.set_var(162, 0);
        }
        138 => {
            pos.y -= px(32);
            *vel = Velocity::default();
            o.ax = 0;
            o.ay = 0;
        }
        139 => {
            pos.y += px(32);
            vel.x = 0;
            vel.y = fp(6.0);
            o.ax = 0;
            o.ay = 0;
            fx.sounds.push(0xAD);
            o.hit_count = 0;
            o.set_var(162, 0);
        }
        140 | 141 => {
            vel.x = fp(if op == 140 { 1.125 } else { -1.125 });
            vel.y = fp(4.0);
            o.ax = 0;
            o.ay = fp(-0.25);
            fx.sounds.push(0xAD);
            o.set_var(162, o.var(162) + 1);
        }
        // I193 (0x16864): boss defeated: flag off, then I192 (room music restarts).
        193 => {
            fx.set_global(BOSS_FLAG, 0);
            fx.set_global(MUSIC_TIMER, 0x11);
        }
        152 | 154 | 156 => {
            match op {
                154 => {
                    o.set_var(162, 1);
                    o.set_var(164, 0x40);
                    o.set_var(166, 0);
                }
                152 => o.set_var(166, 2),
                _ => o.set_var(166, 1),
            }
            copy_mapping(ctx, o, _anim, 0xAC);
        }
        158 | 159 => {
            copy_mapping(ctx, o, _anim, 0x9E);
            if let Some(b) = o.boxes.as_mut() {
                b[0] = 32;
                b[1] = 32;
                b[4] = 32;
                b[5] = 32;
            }
            if op == 159 {
                o.set_var(162, 1);
            }
        }
        194 => {
            o.set_var(158, 0);
            o.flags = 0x4180;
            copy_mapping(ctx, o, _anim, 0xAC);
        }
        207..=214 => {
            let v158 = [0, 10, 20, 30, 5, 15, 25, 35][(op - 207) as usize];
            o.set_var(158, v158);
            o.set_var(160, 0);
            o.set_var(166, 0);
            o.flags = 0x4180;
            copy_mapping(ctx, o, _anim, 0xAC);
        }
        215 => {
            fx.set_global(0xFF0AEC, 1);
            o.set_var(158, 0x37);
            o.flags = 0x4180;
            copy_mapping(ctx, o, _anim, 0xAC);
        }
        231 => {
            fx.sounds.push(0xAA);
            o.set_var(158, 1);
            copy_mapping(ctx, o, _anim, 0xAC);
        }
        19 => {
            vel.x = fp(1.0);
            o.face_left = false;
            fx.persist.push(o.param118);
        }
        21 => {
            vel.x = fp(1.0);
            o.face_left = false;
            fx.set_global(0xFF0AF0, 1);
            fx.persist.push(o.param118);
        }
        82 => {
            if let Some(n) = _p.nearest(pos.px_x()) {
                let (vx, vy) = aim_velocity(direction_code(pos.px_x(), pos.px_y(), n.x, n.y));
                vel.x = fp(vx);
                vel.y = fp(vy);
            }
            o.ax = 0;
            o.ay = 0;
        }
        83 => vel.y = -vel.y,
        84 => {
            o.set_var(158, pos.px_y());
            vel.y = fp(1.0);
            vel.x = 0;
            o.ax = 0;
            o.ay = 0;
        }
        91 | 92 => {
            o.face_left = op == 92;
            o.set_var(158, o.hp);
        }
        107 => {
            o.set_var(158, o.hp);
            o.set_var(160, 1);
            o.set_var(162, 0);
            fx.sounds.push(0xA9);
            o.flags = 16389;
        }
        108 => o.flags |= 0x4001,
        109 => o.flags &= 0xBFFE,
        118 => pos.y -= px(244),
        153 => {
            o.set_var(158, 0);
            o.set_var(160, 0);
        }
        157 => pos.x -= px(8),
        160 | 220 => {
            o.set_var(190, if fx.room == 27 { 0x48 } else { 0 });
            vel.x = fp(-1.0);
            vel.y = 0;
            o.set_var(160, 0x10);
            o.set_var(162, 0x11);
        }
        161..=173 => {
            // Segmented-boss swimming presets (vx negated when v158 is set).
            let (vx, vy, anims, flag176): (f64, Option<f64>, Option<(i32, i32)>, i32) = match op {
                161 => (-0.25, None, Some((0x12, 0x13)), 1),
                162 => (-0.75, Some(0.0), None, 0),
                163 => (-0.375, Some(-0.3125), Some((0x14, 0x15)), 1),
                164 => (-0.625, Some(0.3125), None, 0),
                165 => (-0.5, Some(-0.25), Some((0x14, 0x15)), 1),
                166 => (-0.5, Some(0.25), None, 0),
                167 => (-0.625, Some(-0.1875), Some((0x14, 0x15)), 1),
                168 => (-0.375, Some(0.1875), None, 0),
                169 => (-0.75, Some(-0.125), Some((0x14, 0x15)), 1),
                170 => (-0.25, Some(0.125), None, 0),
                171 => (-0.875, Some(-0.0625), Some((0x14, 0x15)), 1),
                172 => (-0.125, Some(0.0625), None, 0),
                _ => (-1.0, None, Some((0x16, 0x17)), 1),
            };
            let neg = o.var(158) != 0;
            vel.x = fp(if neg { -vx } else { vx });
            if let Some(vy) = vy {
                vel.y = fp(vy);
            }
            if let Some((a, b)) = anims {
                o.set_var(190, if fx.room == 27 { 0x48 } else { 0 });
                o.set_var(160, a);
                o.set_var(162, b);
            }
            o.set_var(176, flag176);
        }
        174 | 179 => {
            if op == 179 {
                o.flags |= flag::BACK_PASS;
            }
            o.flags |= 0x3000;
            let sx = fx.slot(0).map_or(pos.px_x(), |s0| s0.x);
            vel.x = if pos.px_x() < sx { fp(2.0) } else { fp(-2.0) };
            vel.y = 0;
            o.ax = 0;
            o.ay = 0;
        }
        175 => o.flags |= flag::BACK_PASS | 0x3000,
        176 => o.flags &= !flag::BACK_PASS,
        177 | 178 => {
            let right = op == 177;
            vel.x = fp(if right { 3.0 } else { -3.0 });
            if fx.room == 27 {
                vel.y = 0;
                o.ay = 0;
            } else {
                vel.y = fp(-3.0);
                o.ay = fp(0.125);
            }
            o.set_var(182, o.var(182) + 1);
            // v158 (direction) / v186 (anim refresh) of the head itself and its 7 segments.
            o.set_var(158, if right { 1 } else { 0 });
            o.set_var(186, 1);
            for k in 1..=7 {
                let slot = o.slot + k;
                fx.slot_writes.push(SlotWrite::Var { slot, offset: 158, value: if right { 1 } else { 0 } });
                fx.slot_writes.push(SlotWrite::Var { slot, offset: 186, value: 1 });
            }
        }
        180..=186 => {
            let (vx, vy) = match op {
                180 => (-1.875, -1.0),
                181 => (-1.625, -0.875),
                182 => (-1.1875, -0.6875),
                183 => (-0.875, -0.5),
                184 => (-0.5, -0.28125),
                185 => (-0.25, -0.125),
                _ => (-0.125, 0.0),
            };
            o.flags &= 0xCDFF;
            vel.x = fp(if o.var(158) == 0 { -vx } else { vx });
            vel.y = fp(vy);
            o.ax = 0;
            o.ay = 0;
        }
        189 => {
            vel.x = -vel.x;
            vel.y = -vel.y;
            for v in [166, 180, 182, 184, 186, 188] {
                o.set_var(v, 0);
            }
        }
        190 => {
            o.flags = 0x4081;
            if o.ty == 0x10 || o.ty == 0x33 {
                for x in [-80, -48, 48, 80] {
                    spawn_abs(ctx, o, fx, 11, pos.px_x() + 0x80 + x, 304, Pool::Any);
                }
                spawn_abs(ctx, o, fx, 10, pos.px_x() + 0x80, 144, Pool::Any);
                fx.sounds.push(0xA8);
            }
            vel.y = fp(-5.0);
            o.ay = fp(0.125);
            vel.x = fp(match o.ty {
                0x10 | 0x14 | 0x33 | 0x37 => -0.5,
                0x11 | 0x15 | 0x34 | 0x38 => -0.25,
                0x12 | 0x16 | 0x35 | 0x39 => 0.25,
                _ => 0.5,
            });
        }
        197 => o.flags = 0x0180,
        198 => {
            let was = o.var(160) != 0;
            o.set_var(160, if was { 0 } else { 1 });
            pos.x += px(if was { 320 } else { -320 });
        }
        199..=206 => {
            let (vx, vy) = match op {
                199 => (-1.5, -2.5),
                200 => (-2.5, -1.5),
                201 => (1.75, -2.25),
                202 => (1.25, -2.75),
                203 => (-3.0, 1.0),
                204 => (2.0, -2.0),
                205 => (3.0, -3.0),
                _ => (-3.0, -3.0),
            };
            vel.x = fp(vx);
            vel.y = fp(vy);
        }
        216 => {
            spawn_abs(ctx, o, fx, 5, 640, 288, Pool::High);
            fx.sounds.push(0x9A);
        }
        217 => {
            *pos = WorldPos::from_px(688 - 0x80, 384 - 0x80);
            vel.x = 0;
            vel.y = fp(-1.0);
            o.ax = 0;
            o.ay = 0;
        }
        222 => {
            if let Some(s0) = fx.slot(0).copied() {
                for v in [174, 178, 182, 186] {
                    o.set_var(v, s0.x);
                }
                for v in [176, 180, 184, 188] {
                    o.set_var(v, s0.y);
                }
            }
        }
        223 => o.set_var(162, 0),
        227 => {
            o.set_var(160, 1);
            o.set_var(162, 0);
            fx.slot_writes.push(SlotWrite::Var { slot: 5, offset: 162, value: 3 }); // [FF1662] = slot5.v162
            vel.x = 0;
            vel.y = fp(-2.0);
        }
        228 => {
            o.set_var(160, 0);
            o.set_var(162, 0);
            fx.slot_writes.push(SlotWrite::Var { slot: 5, offset: 162, value: 4 });
        }
        230 => o.flags &= !flag::HARMLESS,
        235 => {
            vel.x = if pos.px_x() + 0x80 >= 0x120 { fp(-2.0) } else { fp(2.0) };
            vel.y = fp(-3.0);
            o.ay = fp(0.09375);
        }
        236 => {
            vel.x = ((o.exp as i32) << 16) | o.gold as i32;
            vel.y = ((o.death_type as i32) << 16) | o.defence as i32;
        }
        _ if super::bosses::init(ctx, o, op, pos, vel, _anim, _p, fx) => {}
        _ => unknown(fx, format!("I{op}")),
    }
}

/// Map-space despawn window test (ROM constants minus 0x80).
fn outside(pos: &WorldPos, cam: IVec2, x0: i32, x1: i32, y0: i32, y1: i32) -> bool {
    let (x, y) = (pos.px_x() + 0x80, pos.px_y() + 0x80);
    x < cam.x + x0 || x > cam.x + x1 || y < cam.y + y0 || y > cam.y + y1
}

fn col_half(_anim: &SpriteAnimator, _ctx: &RoomCtx) -> i32 {
    8
}

fn obj_hurt_box(sheet: &SpriteSheet, anim: &SpriteAnimator, pos: &WorldPos) -> Option<HitBox> {
    super::combat::frame_boxes(sheet, anim, pos).0
}

/// Hit boxes of an object: its own copy while its mapping is script-edited, else the frame's.
pub fn object_boxes(o: &Obj, sheet: &SpriteSheet, anim: &SpriteAnimator, pos: &WorldPos) -> (Option<HitBox>, Option<HitBox>) {
    match o.boxes {
        Some(b) => {
            let (x, y) = (pos.px_x(), pos.px_y());
            (HitBox::from_header(x, y, b[0], b[1], b[2], b[3]), HitBox::from_header(x, y, b[4], b[5], b[6], b[7]))
        }
        None => super::combat::frame_boxes(sheet, anim, pos),
    }
}

#[allow(clippy::too_many_arguments)]
fn condition(
    ctx: &RoomCtx,
    o: &mut Obj,
    op: u16,
    pos: &mut WorldPos,
    _vel: &mut Velocity,
    anim: &mut SpriteAnimator,
    c: &Contacts,
    p: &PlayersView,
    fx: &mut Effects,
) -> bool {
    let (x, y) = (pos.px_x(), pos.px_y());
    if let Some(spec) = ctx.ops.cond.get(&op.to_string()) {
        if let Some(t) = spec.timer_eq {
            return o.state_timer == t;
        }
        if let Some((dx, dy)) = spec.near {
            let side_ok = match spec.side {
                None => true,
                Some(Side::Right) => p.nearest(x).is_some_and(|n| n.x >= x),
                Some(Side::Left) => p.nearest(x).is_some_and(|n| n.x < x),
            };
            let r = side_ok && p.near(x, y, dx, dy);
            return r != spec.negate;
        }
    }
    let cam = fx.camera;
    match op {
        0 => c.wall_right,
        1 => c.wall_left,
        2 => !(c.ground || c.on_surface),
        3 => c.ground || c.on_surface,
        4 => true,
        5 => {
            if outside(pos, cam, 0x30, 0x210, 0, 0x1C0) {
                o.delete = true;
            }
            false
        }
        8 => p.nearest(x).is_some_and(|n| n.x >= x),
        9 => p.nearest(x).is_some_and(|n| n.x < x),
        65 => anim.cycled,
        67 => {
            if c.ground || c.ceiling || c.wall_left || c.wall_right {
                o.delete = true;
            }
            o.delete
        }
        68 => o.face_left,
        70 | 71 => {
            let i = (op - 70) as usize;
            let mine = object_boxes(o, ctx.sheet, anim, pos).0;
            p.players.get(i).is_some_and(|pl| {
                pl.alive && matches!((pl.hurt, mine), (Some(a), Some(b)) if a.overlaps(&b))
            })
        }
        72 => p.nearest(x).is_some_and(|n| n.attacking),
        73 => {
            if c.wall_left || c.wall_right {
                fx.sounds.push(0xA8);
                o.pending_death = true;
            }
            false
        }
        74 => {
            let hit = fx
                .others
                .iter()
                .any(|e| e.active && e.slot > o.slot && (e.ty == 0x19 || e.ty == 0x1E) && (e.x - x).abs() < 16 && (e.y - y).abs() < 16);
            if hit {
                fx.sounds.push(0xA8);
                o.pending_death = true;
            }
            false
        }
        78 => {
            if outside(pos, cam, -32, 0x260, 0, 0x1C0) {
                o.delete = true;
            }
            false
        }
        87 => c.ceiling,
        101 => {
            if outside(pos, cam, 0x30, 0x210, 0x78, 0x1C0) {
                o.delete = true;
            }
            false
        }
        102 | 103 => {
            // Shield blocks: attacks hitting the guard box only clang.
            let i = (op - 102) as usize;
            if let Some(pl) = p.players.get(i) {
                if let (Some(atk), Some(hurt)) = (pl.attack, object_boxes(o, ctx.sheet, anim, pos).0) {
                    let guard = HitBox { cx: hurt.cx, cy: y - 40, w: hurt.w, h: 24 };
                    if atk.overlaps(&guard) && !o.hit_by[i] {
                        fx.sounds.push(0xB9);
                        o.hit_by[i] = true;
                    }
                }
            }
            false
        }
        122 | 123 => {
            let i = (op - 122) as usize;
            let spot = HitBox { cx: x, cy: y - 12, w: 24, h: 24 };
            if let Some(pl) = p.players.get(i) {
                if let Some(atk) = pl.attack {
                    if atk.overlaps(&spot) && !o.hit_by[i] {
                        o.hit_by[i] = true;
                        fx.sounds.push(0xA5);
                        o.set_var(160, o.var(160) + 1);
                    }
                }
            }
            o.var(160) >= 5
        }
        124 => magic_hits_spot(o, x, y, fx),
        125 => o.state_timer >= 28 && o.var(162) == 4,
        126 => o.state_timer >= 28 && o.hit_count >= 3,
        127 => o.state_timer >= 28 && o.var(162) >= 4 && p.near(x, y, 0xA0, None),
        135 => {
            if outside(pos, cam, -32, 0x260, 0x48, 0x178) {
                o.delete = true;
            }
            false
        }
        153 => key_flag(ctx, o),
        22 | 23 => {
            let right = op == 23;
            p.nearest(x).is_some_and(|n| (n.x >= x) == right && (n.x - x).abs() <= 0x50)
        }
        75 => p.nearest(x).is_some_and(|n| n.y - 16 < y),
        76 => o.state_timer == o.prev_state_time,
        77 => {
            if o.state_timer == 60 {
                o.delete = true;
            }
            false
        }
        79 => p.nearest(x).is_some_and(|n| n.x == x),
        80 => p.nearest(x).is_some_and(|n| n.facing_right),
        81 => p.nearest(x).is_some_and(|n| !n.facing_right),
        82 | 83 => {
            // Edge probes below the object's sides: no ground ahead.
            let side = if op == 82 { 1 } else { -1 };
            let probe_x = x + side * (col_half(anim, ctx) + 1);
            !super::super::data::attr::is_solid(ctx.def.attr_at(probe_x, y + 1))
        }
        84 => _vel.x == 0,
        86 => o.hp != o.var(158),
        92 | 94 => {
            let i = if op == 92 { 0 } else { 1 };
            let hurt = object_boxes(o, ctx.sheet, anim, pos).0;
            match (p.players.get(i), hurt) {
                (Some(pl), Some(h)) => {
                    let big = HitBox { cx: h.cx, cy: y - 32, w: h.w + 2, h: 64 };
                    pl.alive && pl.hurt.is_some_and(|ph| ph.overlaps(&big))
                }
                _ => false,
            }
        }
        93 | 95 => {
            let i = if op == 93 { 0 } else { 1 };
            let hurt = object_boxes(o, ctx.sheet, anim, pos).0;
            match (p.players.get(i), hurt) {
                (Some(pl), Some(h)) => {
                    let guard = HitBox { cx: h.cx, cy: y - 40, w: h.w, h: 24 };
                    pl.attack.is_some_and(|a| a.overlaps(&guard))
                }
                _ => false,
            }
        }
        96 | 97 => {
            let right = op == 96;
            p.nearest(x).is_some_and(|n| (n.x >= x) == right && (n.y - y).abs() <= 0x40)
        }
        98 | 99 => {
            let right = op == 98;
            p.nearest(x).is_some_and(|n| {
                let d = (n.x - x).abs();
                (n.x >= x) == right && d <= 0x50 && d > 0x58
            })
        }
        100 => o.var(160) == 0,
        104..=111 => {
            let Some(n) = p.nearest(x) else { return false };
            let d = direction_code(x, y, n.x, n.y - 48);
            match op {
                104 => d == 1,
                105 => [5, 9, 13].contains(&d),
                106 => d == 7,
                107 => [3, 11, 15].contains(&d),
                108 => d == 2,
                109 => [6, 10, 14].contains(&d),
                110 => d == 4,
                _ => [0, 8, 12].contains(&d),
            }
        }
        112 => {
            if o.slot == 0 || fx.slot(o.slot - 1).is_none() {
                o.delete = true;
            }
            false
        }
        113 => {
            if o.slot > 0 && fx.slot(o.slot - 1).is_some_and(|prev| prev.ty < 15) {
                o.pending_death = true;
            }
            false
        }
        114..=118 => o.var(162) == (op - 112) as i32,
        121 => {
            if o.slot > 0 {
                if let Some(prev) = fx.slot(o.slot - 1) {
                    o.flash = prev.flash;
                }
            }
            false
        }
        128 => o.state_timer == 80,
        129 => o.state_timer == 206,
        131 => o.state_timer == 104,
        132 | 133 => {
            let right = op == 132;
            o.state_timer == 104 && p.nearest(x).is_some_and(|n| (n.x >= x) == right && (n.x - x).abs() <= 0x20)
        }
        134 => fx.slot(1).is_none() && fx.slot(2).is_none(),
        136 => {
            let s0 = fx.slot(0).map_or(0, |s| s.var(158));
            let s7 = fx.slot(7).map_or(0, |s| s.var(158));
            let s0_166 = fx.slot(0).map_or(0, |s| s.var(166));
            o.state_timer >= 44 && s0 == s7 && s0_166 != 0
        }
        137 => fx.slot(0).is_some_and(|s0| {
            let (a, b) = (x.min(o.var(178)), x.max(o.var(178)));
            (a..=b).contains(&s0.x)
        }),
        138 => o.var(158) == 1,
        139 => o.var(182) == 2 && (c.wall_left || c.wall_right),
        140 => o.var(184) != 0,
        141 => o.state_timer == 116,
        142..=148 => o.state_timer == 18 + 8 * (op - 142) as u32,
        149 => {
            let hits = fx.slot(0).map_or(0, |s0| s0.hit_count);
            hits >= if fx.room < 20 { 10 } else { 20 }
        }
        150 => {
            if outside(pos, cam, -32, 0x260, -32, 0x1E0) {
                o.delete = true;
            }
            false
        }
        151 => o.var(158) == 0x36,
        152 => o.var(158) == 0,
        154 => cam.x > 0x4B0,
        155 => (x + 0x80) << 16 > (((o.exp as i32) << 16) | o.gold as i32),
        156 => fx.global(0xFF0D46) != 0 && fx.global(BOSS_FLAG) == 0,
        157 | 158 => {
            let i = (op - 157) as usize;
            let mine = object_boxes(o, ctx.sheet, anim, pos).0;
            p.players.get(i).is_some_and(|pl| matches!((pl.attack, mine), (Some(a), Some(b)) if a.overlaps(&b)))
        }
        159 => y + 0x80 < 0x140,
        162 | 163 => {
            if fx.room == 7 && fx.global(0xFF0AF0) != 0 {
                false
            } else {
                // C30/C31: player contact variants.
                let i = (op - 162) as usize;
                let mine = object_boxes(o, ctx.sheet, anim, pos).0;
                p.players.get(i).is_some_and(|pl| pl.alive && matches!((pl.hurt, mine), (Some(a), Some(b)) if a.overlaps(&b)))
            }
        }
        165 => o.hp < 0x8000,
        166 => fx.slot(0).map_or(0, |s| s.var(160)) == 0 && (p.nearest(x).is_some_and(|n| n.x >= x) || p.near(x, y, 0x80, None)),
        167 => {
            if y + 0x80 < 0xE0 {
                pos.y = px(0xE0 - 0x80);
                true
            } else {
                false
            }
        }
        168 => {
            if y + 0x80 >= 0x130 {
                pos.y = px(0x130 - 0x80);
                fx.sounds.push(0xB0);
                true
            } else {
                false
            }
        }
        171 => o.hp < 0x7A00,
        172 => y + 0x80 < 0x94,
        173 => fx.slot(0).is_some_and(|s0| s0.var(164) != 0), // [FF1164] = slot0.v164
        160 | 161 => {
            let i = (op - 160) as usize;
            let mine = object_boxes(o, ctx.sheet, anim, pos).0;
            key_flag(ctx, o)
                && p.players.get(i).is_some_and(|pl| {
                    pl.alive && matches!((pl.hurt, mine), (Some(a), Some(b)) if a.overlaps(&b))
                })
        }
        _ => match super::bosses::condition(ctx, o, op, pos, _vel, anim, c, p, fx) {
            Some(r) => r,
            None => {
                unknown(fx, format!("C{op}"));
                false
            }
        },
    }
}

/// C124 (0x1BAB0): the Mage's spell hits the hidden-item spot's 24×24 box; true on the 5th hit
/// (docs/re/hidden_items.md). The spot's +144 timer is only counted down here, by the first live
/// spell object of each tick, so one projectile normally scores once.
fn magic_hits_spot(o: &mut Obj, x: i32, y: i32, fx: &mut Effects) -> bool {
    let sv = &fx.spells;
    if !sv.active {
        return false;
    }
    // Explosion: every tick of the spell scores, without position or timer tests.
    let hit = sv.spell == 5 || {
        let n = if sv.spell == 4 { 16 } else { 8 };
        let spot = HitBox { cx: x, cy: y - 12, w: 24, h: 24 };
        let mut hit = false;
        for (_, b) in sv.boxes.iter().filter(|(slot, _)| *slot < n) {
            if o.invuln != 0 {
                o.invuln -= 1;
                return false;
            }
            if b.is_some_and(|b| b.overlaps(&spot)) {
                hit = true;
                break;
            }
        }
        hit
    };
    if !hit {
        return false;
    }
    fx.sounds.push(0xA5);
    o.invuln = 20;
    o.set_var(160, o.var(160) + 1);
    o.var(160) == 5
}

/// The flag byte addressed by death_type:defence (e.g. the silver key) is set.
fn key_flag(ctx: &RoomCtx, o: &Obj) -> bool {
    let addr = ((o.death_type as u32) << 16) | o.defence as u32;
    ctx.flags.get(&addr).copied().unwrap_or(0) != 0
}

/// Turns the object into its death type (puff, then drop) — cartridge 0x144C6.
#[allow(clippy::too_many_arguments)]
pub fn become_death_type(
    ctx: &RoomCtx,
    o: &mut Obj,
    pos: &mut WorldPos,
    vel: &mut Velocity,
    anim: &mut SpriteAnimator,
    col: &mut Collider,
    p: &PlayersView,
    fx: &mut Effects,
) {
    o.pending_death = false;
    // The kill clears the hit-flash counter (0x144E6) without restoring a boss flash palette.
    o.flash = 0;
    o.ty = o.death_type;
    o.tile_ofs = 0;
    o.flags |= flag::UNHITTABLE | flag::HARMLESS;
    enter_state(ctx, o, 0, pos, vel, anim, col, p, fx);
}

/// Applies queued script effects.
#[allow(clippy::too_many_arguments)]
pub fn apply_effects(
    mut fx: ResMut<Effects>,
    mut queue: ResMut<SpawnQueue>,
    mut persist: ResMut<super::PersistFlags>,
    mut world: ResMut<super::ObjectWorld>,
    mut dialog: ResMut<super::super::dialog::Dialog>,
    mut objs: Query<&mut Obj>,
) {
    queue.0.append(&mut fx.spawns);
    for f in fx.persist.drain(..) {
        persist.0[(f & 0xFF) as usize] = 0xFF;
    }
    for m in fx.messages.drain(..) {
        dialog.open(m, 1);
    }
    world.budget = world.budget.saturating_sub(std::mem::take(&mut fx.budget_refund));
    if let Some((a, b)) = fx.clear_slots.take() {
        for mut o in &mut objs {
            if (a..=b).contains(&o.slot) {
                o.delete = true;
            }
        }
    }
    let writes = std::mem::take(&mut fx.slot_writes);
    for mut o in &mut objs {
        for w in &writes {
            match *w {
                SlotWrite::Var { slot, offset, value } if slot == o.slot => o.set_var(offset, value),
                SlotWrite::State { slot, state } if slot == o.slot => o.pending_state = Some(state),
                SlotWrite::Delete { slot } if slot == o.slot => o.delete = true,
                SlotWrite::Flags { slot, and, or } if slot == o.slot => o.flags = (o.flags & and) | or,
                _ => {}
            }
        }
    }
    if std::mem::take(&mut fx.delete_all) {
        for mut o in &mut objs {
            o.delete = true;
        }
    }
    let _ = SpawnRecord::default();
}
