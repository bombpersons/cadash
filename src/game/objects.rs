//! Non-player objects (enemies, NPCs, items, chests, projectiles, bosses).
//!
//! Every object is a small state machine described by data (docs/re/objects.md §2): each state
//! names an animation, a per-tick *behaviour* opcode, an *init* opcode run on entry and a list of
//! *condition → next state* pairs. Simple opcodes are table driven (`objects/opcodes.ops.json`);
//! the rest are implemented in [`ops`].
use super::{
    AppState,
    anim::{self, SpriteAnimator, animated_sprite},
    camera::CameraPos,
    collision::{Collider, Contacts},
    coords::{Velocity, WorldPos, z},
    data::{GameData, OpcodeTable, RoomDef, RoomObjects, SpawnRecord, SpriteSheet},
    room::{CurrentRoom, RoomEntity},
    sprite_palette,
};
use crate::GameSystems;
use bevy::prelude::*;

pub mod bosses;
pub mod combat;
pub mod ops;

pub const SLOTS: usize = 14;
/// Script variable words from +158 to the end of the 0x100-byte slot.
pub const VARS: usize = 49;
/// Maximum summed object cost in a room at once (0xFF0B54 budget).
pub const BUDGET: u32 = 40;

/// Object flag bits (+102).
pub mod flag {
    pub const HARMLESS: u16 = 1 << 0;
    pub const PICKUP: u16 = 1 << 1;
    pub const POISON: u16 = 1 << 2;
    pub const BLOCKABLE: u16 = 1 << 3;
    pub const PERSIST_ON_PICKUP: u16 = 1 << 4;
    pub const NPC: u16 = 1 << 5;
    pub const SOLID: u16 = 1 << 6;
    pub const BOSS: u16 = 1 << 7;
    pub const HIGH_PRIORITY: u16 = 1 << 8;
    pub const BACK_PASS: u16 = 1 << 9;
    pub const BOSS_PART: u16 = 1 << 10;
    pub const CLANG: u16 = 1 << 11;
    pub const MAGIC_PROOF: u16 = 1 << 12;
    pub const MELEE_PROOF: u16 = 1 << 13;
    pub const UNHITTABLE: u16 = 1 << 14;
    pub const PLATFORM: u16 = 1 << 15;
}

/// Runtime state of one object.
#[derive(Component, Debug, Clone)]
pub struct Obj {
    pub slot: usize,
    pub ty: u16,
    pub state: u16,
    pub hp: i32,
    pub exp: u16,
    pub gold: u16,
    pub death_type: u16,
    pub defence: u16,
    pub flags: u16,
    pub touch_dmg: u16,
    pub attack_dmg: u16,
    pub tile_ofs: u16,
    pub param118: u16,
    pub cost: u16,
    /// Acceleration (16.16 px/tick²).
    pub ax: i32,
    pub ay: i32,
    pub face_left: bool,
    pub anim_id: Option<u32>,
    pub gfx: String,
    pub state_timer: u32,
    pub prev_state_time: u32,
    pub age: u32,
    pub hit_count: u32,
    /// Already hit by player 1/2 during their current swing.
    pub hit_by: [bool; 2],
    /// Magic-projectile invulnerability ticks (+144).
    pub invuln: u32,
    /// Hit flash ticks (+145): non-bosses freeze and blink.
    pub flash: u32,
    /// Blink-out timer for dropped items (+154).
    pub despawn_timer: u32,
    /// Script scratch variables v158.. (index = (offset - 158) / 2).
    pub vars: [i32; VARS],
    /// Marker this object came from (re-armed on deletion).
    pub marker: Option<usize>,
    pub delete: bool,
    /// Set by conditions that make the object explode; handled after the tick.
    pub pending_death: bool,
    /// The death came from a melee hit this tick (0x14366, before the animation step and script
    /// tick), so the death type already animates and runs its script in the same tick.
    pub hit_kill: bool,
    /// State forced by another object's script; entered at the start of the next tick.
    pub pending_state: Option<u16>,
    /// Offset of an editable mapping copy inside the variables (+0xAC or +0x9E), if any.
    pub ram_map: Option<usize>,
    /// Hit boxes (hurt w,h,dx,dy, attack w,h,dx,dy) owned by the object while its mapping is
    /// edited by script (cartridge +46..+60).
    pub boxes: Option<[i16; 8]>,
}

impl Obj {
    pub fn var(&self, offset: usize) -> i32 {
        self.vars.get((offset - 158) / 2).copied().unwrap_or(0)
    }
    pub fn set_var(&mut self, offset: usize, v: i32) {
        if let Some(slot) = self.vars.get_mut((offset - 158) / 2) {
            *slot = (v as i16) as i32;
        }
    }
    /// Pieces of the editable mapping: (dx, dy, size, attr).
    pub fn ram_pieces(&self) -> Vec<[i32; 4]> {
        let Some(base) = self.ram_map else { return vec![] };
        let count = self.var(base).max(0) as usize;
        (0..count)
            .map(|k| {
                let o = base + 18 + 8 * k;
                [self.var(o), self.var(o + 2), self.var(o + 4) & 0xFF, self.var(o + 6) & 0xFFFF]
            })
            .filter(|p| p[0] & 0xFFFF != 0x8000)
            .collect()
    }
    fn from_record(slot: usize, rec: &SpawnRecord, marker: Option<usize>) -> Self {
        Self {
            slot,
            ty: rec.ty,
            state: 0,
            hp: rec.hp as i32,
            exp: rec.exp,
            gold: rec.gold,
            death_type: rec.death_type,
            defence: rec.defence,
            flags: rec.flags,
            touch_dmg: rec.touch_dmg,
            attack_dmg: rec.attack_dmg,
            tile_ofs: rec.tile_ofs,
            param118: rec.param118,
            cost: rec.cost,
            ax: 0,
            ay: 0,
            face_left: false,
            anim_id: None,
            gfx: "default".into(),
            state_timer: 0,
            prev_state_time: 0,
            age: 0,
            hit_count: 0,
            hit_by: [false; 2],
            invuln: 0,
            flash: 0,
            despawn_timer: 0,
            vars: [0; VARS],
            marker,
            delete: false,
            pending_death: false,
            hit_kill: false,
            pending_state: None,
            ram_map: None,
            boxes: None,
        }
    }
}

/// Which slots are in use, the cost budget, and the room's marker states.
#[derive(Resource, Default, Debug)]
pub struct ObjectWorld {
    pub slots: [Option<Entity>; SLOTS],
    pub budget: u32,
    /// Marker armed (true) or consumed by a live object.
    pub armed: Vec<bool>,
    /// Marker cells → marker index.
    pub cell_markers: std::collections::HashMap<(i32, i32), usize>,
    /// Last camera block position used for spawn scanning.
    pub last_cam_block: Option<IVec2>,
}

/// Flags that keep collected chests, opened doors and defeated bosses gone (0xFF0D40).
#[derive(Resource, Debug)]
pub struct PersistFlags(pub [u8; 256]);

impl Default for PersistFlags {
    fn default() -> Self {
        Self([0; 256])
    }
}

/// Requests to create objects from spawn records (by scripts or markers).
#[derive(Resource, Default, Debug)]
pub struct SpawnQueue(pub Vec<SpawnRequest>);

#[derive(Debug, Clone)]
pub struct SpawnRequest {
    pub record: SpawnRecord,
    pub x: i32,
    pub y: i32,
    pub pool: Pool,
    pub marker: Option<usize>,
    pub gfx: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    /// Slots 0..=6 (normal markers).
    Low,
    /// Slots 7..=13 (high-pool markers, script spawns).
    High,
    /// Any slot.
    Any,
    /// Persistent records: slots 6→0 then 13→7.
    Persistent,
}

fn slot_order(pool: Pool) -> Vec<usize> {
    match pool {
        Pool::Low => (0..7).collect(),
        Pool::High => (7..14).collect(),
        Pool::Any => (0..14).collect(),
        Pool::Persistent => (0..7).rev().chain((7..14).rev()).collect(),
    }
}

/// Animation key in the room's object sheet.
pub fn anim_key(anim: u32, tile_ofs: u16, gfx: &str) -> String {
    let ofs = if anim != 0 { tile_ofs } else { 0 };
    format!("{anim}:{ofs}:{gfx}")
}

/// Handles to the active room's object data.
pub struct RoomCtx<'a> {
    pub def: &'a RoomDef,
    pub objs: &'a RoomObjects,
    pub sheet: &'a SpriteSheet,
    pub ops: &'a OpcodeTable,
    pub data: &'a GameData,
    /// Story/item flags keyed by their cartridge RAM address.
    pub flags: &'a std::collections::HashMap<u32, u8>,
}

#[derive(Resource)]
pub struct OpcodeHandle(pub Handle<OpcodeTable>);

fn load_ops(mut commands: Commands, server: Res<AssetServer>) {
    commands.insert_resource(OpcodeHandle(server.load("objects/opcodes.ops.json")));
}

/// Resets object state for a newly entered room.
pub fn reset_for_room(world: &mut ObjectWorld, objs: &RoomObjects) {
    world.slots = [None; SLOTS];
    world.budget = 0;
    world.armed = vec![true; objs.markers.len()];
    world.cell_markers = objs
        .markers
        .iter()
        .enumerate()
        .map(|(i, m)| ((m.cell[0], m.cell[1]), i))
        .collect();
    world.last_cam_block = None;
}

fn marker_request(objs: &RoomObjects, persist: &PersistFlags, index: usize) -> Option<SpawnRequest> {
    let m = objs.markers.get(index)?;
    if let Some(f) = m.guard_flag {
        if persist.0[f as usize] != 0 {
            return None;
        }
    }
    let record = if m.persistent { objs.persistent_records.get(m.index)? } else { objs.records.get(m.index)? };
    let pool = if m.persistent {
        Pool::Persistent
    } else if m.high_pool {
        Pool::High
    } else {
        Pool::Low
    };
    Some(SpawnRequest {
        record: *record,
        x: m.cell[0] * 16 + 16,
        y: m.cell[1] * 16 + 16,
        pool,
        marker: Some(index),
        gfx: "default".into(),
    })
}

/// Spawns markers that scroll into view (cartridge 0x13CE2/0x13D00/0x13DDA/0x14ABA).
pub fn scan_markers(
    cam: Res<CameraPos>,
    room: Option<Res<CurrentRoom>>,
    rooms: Res<Assets<RoomDef>>,
    objects: Res<Assets<RoomObjects>>,
    persist: Res<PersistFlags>,
    mut world: ResMut<ObjectWorld>,
    mut queue: ResMut<SpawnQueue>,
) {
    let Some(def) = room.and_then(|r| rooms.get(&r.def)) else { return };
    let Some(objs) = objects.get(&def.objects_handle) else { return };
    if world.armed.len() != objs.markers.len() {
        reset_for_room(&mut world, objs);
    }
    let block = IVec2::new(cam.x >> 4, cam.y >> 4);
    let mut cells: Vec<(i32, i32)> = vec![];
    match world.last_cam_block {
        None => {
            // Room entry: a wide window around the screen.
            for cx in block.x - 4..=block.x + 28 {
                for cy in block.y - 2..block.y + 15 {
                    cells.push((cx, cy));
                }
            }
        }
        Some(last) => {
            if last.x != block.x {
                for cx in [block.x - 2, block.x + 22] {
                    for cy in block.y - 2..block.y + 14 {
                        cells.push((cx, cy));
                    }
                }
            }
            if last.y != block.y {
                for cy in [block.y - 2, block.y + 13] {
                    for cx in block.x - 2..=block.x + 22 {
                        cells.push((cx, cy));
                    }
                }
            }
        }
    }
    world.last_cam_block = Some(block);
    for c in cells {
        if let Some(&i) = world.cell_markers.get(&c) {
            if world.armed[i] {
                if let Some(req) = marker_request(objs, &persist, i) {
                    queue.0.push(req);
                }
            }
        }
    }
}

/// Creates queued objects in free slots within the cost budget (cartridge 0x14060).
#[allow(clippy::too_many_arguments)]
pub fn spawn_queued(
    mut commands: Commands,
    room: Option<Res<CurrentRoom>>,
    rooms: Res<Assets<RoomDef>>,
    objects: Res<Assets<RoomObjects>>,
    sheets: Res<Assets<SpriteSheet>>,
    opsh: Res<OpcodeHandle>,
    optabs: Res<Assets<OpcodeTable>>,
    datas: Res<super::GameDataHandle>,
    gdata: Res<Assets<GameData>>,
    mut world: ResMut<ObjectWorld>,
    mut queue: ResMut<SpawnQueue>,
    mut fx: ResMut<ops::Effects>,
    story: Res<super::dialog::StoryFlags>,
    players: Query<ops::PlayerQueryData, Without<Obj>>,
) {
    if queue.0.is_empty() {
        return;
    }
    // Init opcodes read the room number (I131's boss caption), so it must be current on room entry.
    fx.room = room.as_ref().map_or(0, |r| r.index);
    let Some(def) = room.and_then(|r| rooms.get(&r.def)) else { return };
    let (Some(objs), Some(ops_t), Some(data)) =
        (objects.get(&def.objects_handle), optabs.get(&opsh.0), gdata.get(&datas.0))
    else {
        return;
    };
    let Some(sheet) = sheets.get(&objs.sheet_handle) else { return };
    let ctx = RoomCtx { def, objs, sheet, ops: ops_t, data, flags: &story.0 };
    for req in std::mem::take(&mut queue.0) {
        if world.budget + req.record.cost as u32 > BUDGET {
            continue;
        }
        let Some(slot) = slot_order(req.pool).into_iter().find(|&s| world.slots[s].is_none()) else { continue };
        let mut obj = Obj::from_record(slot, &req.record, req.marker);
        obj.gfx = req.gfx.clone();
        let mut pos = WorldPos::from_px(req.x, req.y);
        let mut vel = Velocity::default();
        let mut animator = SpriteAnimator::new(objs.sheet_handle.clone(), "", z::ACTORS);
        animator.manual = true;
        animator.first_frame_full = true;
        let mut col = Collider::default();
        // State-0 init opcodes may aim at the players (I82, I225).
        let p = ops::PlayersView::collect(&players, sheets.as_ref());
        ops::enter_state(&ctx, &mut obj, 0, &mut pos, &mut vel, &mut animator, &mut col, &p, &mut fx);
        let base_attr_high = obj.flags & flag::HIGH_PRIORITY != 0;
        let entity = commands
            .spawn((
                Name::new(format!("obj{slot} t{}", obj.ty)),
                obj,
                pos,
                vel,
                col,
                Contacts::default(),
                animated_sprite(animator),
                RoomEntity,
                ObjDepth(base_attr_high),
            ))
            .id();
        world.slots[slot] = Some(entity);
        world.budget += req.record.cost as u32;
        if let Some(m) = req.marker {
            if let Some(a) = world.armed.get_mut(m) {
                *a = false;
            }
        }
    }
}

/// Sprite-priority hint (VDP high priority draws above high-priority foreground).
#[derive(Component)]
pub struct ObjDepth(pub bool);

/// Runs every object's tick in slot order (cartridge 0x14272).
#[allow(clippy::too_many_arguments)]
pub fn tick_objects(
    room: Option<Res<CurrentRoom>>,
    rooms: Res<Assets<RoomDef>>,
    objects: Res<Assets<RoomObjects>>,
    sheets: Res<Assets<SpriteSheet>>,
    opsh: Res<OpcodeHandle>,
    optabs: Res<Assets<OpcodeTable>>,
    datas: Res<super::GameDataHandle>,
    gdata: Res<Assets<GameData>>,
    cam: Res<CameraPos>,
    world: Res<ObjectWorld>,
    story: Res<super::dialog::StoryFlags>,
    spells: Res<super::magic::SpellView>,
    mut fx: ResMut<ops::Effects>,
    players: Query<ops::PlayerQueryData, Without<Obj>>,
    mut q: Query<(&mut Obj, &mut WorldPos, &mut Velocity, &mut SpriteAnimator, &mut Collider, &mut Contacts)>,
) {
    let room_index = room.as_ref().map_or(0, |r| r.index);
    let Some(def) = room.and_then(|r| rooms.get(&r.def)) else { return };
    let (Some(objs), Some(ops_t), Some(data)) =
        (objects.get(&def.objects_handle), optabs.get(&opsh.0), gdata.get(&datas.0))
    else {
        return;
    };
    let Some(sheet) = sheets.get(&objs.sheet_handle) else { return };
    let ctx = RoomCtx { def, objs, sheet, ops: ops_t, data, flags: &story.0 };
    fx.room = room_index;
    let pv = ops::PlayersView::collect(&players, sheets.as_ref());
    fx.camera = IVec2::new(cam.x, cam.y);
    fx.spells = spells.clone();
    for slot in 0..SLOTS {
        let Some(e) = world.slots[slot] else { continue };
        // Live view of every other object for cross-slot conditions (cartridge reads RAM directly).
        fx.others = q
            .iter()
            .map(|(o, p, _, a, ..)| ops::OtherObj {
                slot: o.slot,
                ty: o.ty,
                x: p.px_x(),
                y: p.px_y(),
                state: o.state,
                active: !o.delete,
                hp: o.hp,
                hit_count: o.hit_count,
                flash: o.flash,
                vars: o.vars,
                wx: p.x,
                wy: p.y,
                piece0: bosses::first_piece(&o, sheet, a),
            })
            .collect();
        for req in fx.spawns.clone() {
            fx.predict_spawn(&req);
        }
        let Ok((mut o, mut pos, mut vel, mut animator, mut col, mut c)) = q.get_mut(e) else { continue };
        if o.delete {
            continue;
        }
        if o.pending_death {
            ops::become_death_type(&ctx, &mut o, &mut pos, &mut vel, &mut animator, &mut col, &pv, &mut fx);
            if !std::mem::take(&mut o.hit_kill) {
                continue;
            }
            // set_anim (timer = duration) followed by this tick's step: the first frame of the death
            // animation lasts duration - 1 ticks.
            anim::step(sheet, &mut animator);
        }
        let gfx_before = o.gfx.clone();
        if let Some(s) = o.pending_state.take() {
            ops::enter_state(&ctx, &mut o, s, &mut pos, &mut vel, &mut animator, &mut col, &pv, &mut fx);
        }
        // Hit flash counter (+145, set to 1 by a hit; 0x14294). Ordinary objects stay frozen while it
        // counts 1..13; bosses (flags bit 7) keep acting and brighten palette line 2 from count 1 until
        // it reaches 6 (0x142F2). `object_visuals` hides objects on even counts.
        if o.flash > 0 && o.flags & flag::BOSS == 0 {
            o.flash = (o.flash + 1) % 14;
            sprite_palette::note_context_change(&mut fx, &gfx_before, &o.gfx);
            continue;
        }
        if o.flash > 0 {
            if o.flash == 1 {
                fx.palette_ops.push(sprite_palette::PaletteOp::FlashOn);
            }
            o.flash = (o.flash + 1) % 6;
            if o.flash == 0 {
                fx.palette_ops.push(sprite_palette::PaletteOp::FlashOff);
            }
        }
        anim::step(sheet, &mut animator);
        ops::tick(&ctx, &mut o, &mut pos, &mut vel, &mut animator, &mut col, &mut c, &pv, &mut fx);
        sprite_palette::note_context_change(&mut fx, &gfx_before, &o.gfx);
        o.age += 1;
        o.state_timer += 1;
        if o.despawn_timer > 0 {
            o.despawn_timer += 1;
            if o.despawn_timer >= 540 {
                o.delete = true;
            }
        }
        drop((o, pos, vel, animator, col, c));
        // Writes to other slots take effect before the next object runs.
        let writes = std::mem::take(&mut fx.slot_writes);
        for w in writes {
            type Apply = Box<dyn Fn(&mut Obj, &mut WorldPos)>;
            let (target, apply): (usize, Apply) = match w {
                ops::SlotWrite::Var { slot, offset, value } => (slot, Box::new(move |t, _| t.set_var(offset, value))),
                ops::SlotWrite::State { slot, state } => (slot, Box::new(move |t, _| t.pending_state = Some(state))),
                ops::SlotWrite::Delete { slot } => (slot, Box::new(|t, _| t.delete = true)),
                ops::SlotWrite::Pos { slot, x, y } => (slot, Box::new(move |_, p| *p = WorldPos { x, y })),
                ops::SlotWrite::Flags { slot, and, or } => (slot, Box::new(move |t, _| t.flags = (t.flags & and) | or)),
            };
            if let Some(te) = world.slots.get(target).copied().flatten() {
                if let Ok((mut t, mut p, ..)) = q.get_mut(te) {
                    apply(&mut t, &mut p);
                }
            }
        }
    }
}

/// Deletes objects flagged for deletion, re-arming their markers (cartridge 0x1487C).
pub fn reap_objects(
    mut commands: Commands,
    mut world: ResMut<ObjectWorld>,
    q: Query<(Entity, &Obj, Option<&PieceSprites>)>,
) {
    for (e, o, pieces) in &q {
        if !o.delete {
            continue;
        }
        if let Some(p) = pieces {
            for c in &p.0 {
                commands.entity(*c).despawn();
            }
        }
        if world.slots[o.slot] == Some(e) {
            world.slots[o.slot] = None;
        }
        world.budget = world.budget.saturating_sub(o.cost as u32);
        if o.flags & flag::BOSS == 0 {
            if let Some(m) = o.marker {
                if let Some(a) = world.armed.get_mut(m) {
                    *a = true;
                }
            }
        }
        commands.entity(e).despawn();
    }
}

/// Sprites drawing an object's script-edited mapping piece by piece.
#[derive(Component, Default)]
pub struct PieceSprites(pub Vec<Entity>);

/// Draws objects whose mapping lives in their variables (multi-part bosses, chains).
pub fn render_pieces(
    mut commands: Commands,
    sheets: Res<Assets<SpriteSheet>>,
    mut q: Query<(Entity, &Obj, &WorldPos, &SpriteAnimator, Option<&mut PieceSprites>)>,
    mut sprites: Query<(&mut Sprite, &mut Transform), Without<Obj>>,
) {
    for (e, o, pos, anim, pieces) in &mut q {
        if o.ram_map.is_none() {
            // The mapping went back to ROM (new anim): hide the piece sprites.
            for child in pieces.iter().flat_map(|p| p.0.iter()) {
                if let Ok((mut s, _)) = sprites.get_mut(*child) {
                    s.rect = Some(Rect::new(0.0, 0.0, 0.0, 0.0));
                }
            }
            continue;
        }
        let Some(sheet) = sheets.get(&anim.sheet) else { continue };
        let list = if hit_flash_hidden(o) { Vec::new() } else { o.ram_pieces() };
        let Some(mut have) = pieces else {
            commands.entity(e).insert(PieceSprites::default());
            continue;
        };
        while have.0.len() < list.len() {
            let child = commands.spawn((Sprite::default(), bevy::sprite::Anchor::TOP_LEFT, Transform::default(), RoomEntity)).id();
            have.0.push(child);
        }
        for (k, child) in have.0.iter().enumerate() {
            let Ok((mut s, mut tf)) = sprites.get_mut(*child) else { continue };
            let Some(p) = list.get(k) else {
                s.rect = Some(Rect::new(0.0, 0.0, 0.0, 0.0));
                continue;
            };
            let attr = p[3] as u16;
            let key = format!("{}:{}:{}", attr.wrapping_add(if o.anim_id.unwrap_or(0) != 0 { 0 } else { 0 }), p[2], o.gfx);
            let Some(&fi) = sheet.pieces.get(&key).or_else(|| sheet.pieces.get(&format!("{}:{}:default", attr, p[2]))) else {
                s.rect = Some(Rect::new(0.0, 0.0, 0.0, 0.0));
                continue;
            };
            let [x, y, w, h] = sheet.frames[fi].rect;
            s.image = sheet.image.clone();
            s.rect = Some(Rect::new(x as f32, y as f32, (x + w) as f32, (y + h) as f32));
            tf.translation = super::coords::to_bevy(pos.px_x() + p[0], pos.px_y() + p[1], anim.z + 0.001 * (list.len() - k) as f32);
        }
    }
}

/// Hit-flash blink (0x1495E): while the counter runs, objects (bosses too) are drawn only on odd counts.
pub fn hit_flash_hidden(o: &Obj) -> bool {
    o.flash != 0 && o.flash % 2 == 0
}

/// Visibility (dropped-item blink, hit flash blink) and draw depth by slot.
pub fn object_visuals(mut q: Query<(&Obj, &ObjDepth, &mut Visibility, &mut SpriteAnimator)>) {
    for (o, depth, mut vis, mut a) in &mut q {
        let blink = (o.despawn_timer >= 360 && o.despawn_timer % 2 == 1) || hit_flash_hidden(o);
        // Script-edited mappings are drawn by `render_pieces` instead of the frame image.
        *vis = if blink || o.ram_map.is_some() { Visibility::Hidden } else { Visibility::Inherited };
        // Lower slots draw on top; the second pass (flag bit 9) goes behind the first.
        let pass = if o.flags & flag::BACK_PASS != 0 { 0.0 } else { 0.5 };
        let base = if depth.0 { z::FOREGROUND_HIGH + 1.0 } else { z::ACTORS };
        a.z = base + pass + (SLOTS - o.slot) as f32 * 0.01;
    }
}

pub fn clear_objects(world: &mut ObjectWorld) {
    world.slots = [None; SLOTS];
    world.budget = 0;
    world.armed.clear();
    world.cell_markers.clear();
    world.last_cam_block = None;
}

pub fn objects_running(
    dialog: Res<super::dialog::Dialog>,
    windows: Res<super::dialog::Windows>,
    transition: Option<Res<super::transition::Transition>>,
    freeze: Res<super::room_events::WorldFreeze>,
) -> bool {
    !dialog.is_open() && !windows.any_open() && transition.is_none() && !freeze.0
}

pub fn plugin(app: &mut App) {
    app.init_resource::<ObjectWorld>()
        .init_resource::<PersistFlags>()
        .init_resource::<SpawnQueue>()
        .init_resource::<ops::Effects>()
        .add_systems(Startup, load_ops)
        .add_systems(
            FixedUpdate,
            (
                // Contact with objects is tested in the player update (0x9FF8), against where the
                // objects were before they move this tick.
                combat::objects_touch_players,
                // Scrolling spawns happen before the object update (0x13CE2 before 0x14272), so a
                // new object already moves on the tick it appears.
                scan_markers,
                spawn_queued,
                combat::player_hits_objects,
                tick_objects,
                ops::apply_effects,
                reap_objects,
                // Objects created by scripts during the update.
                spawn_queued,
            )
                .chain()
                .in_set(GameSystems::Game)
                .after(super::player::tick)
                .run_if(in_state(AppState::InGame))
                .run_if(objects_running),
        )
        .add_systems(Update, object_visuals)
        .add_systems(PostUpdate, (render_pieces, bosses::render_plane_b.before(super::room::scroll_background)).before(TransformSystems::Propagate));
}
