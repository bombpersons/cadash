//! The Mage's magic (docs/re/magic.md; numbers in `data/magic.data.json`).
//!
//! Hold B to charge: after 30 ticks a bubble shows the selected spell, which cycles every 30 ticks;
//! releasing B casts it if there is enough MP. Only one spell exists at a time. Spells are
//! projectile entities; their hits on objects ignore defence. In a two-player game the Mage is
//! whichever player plays that class (the classes always differ).
use super::{
    AppState, GameDataRef,
    anim::{self, SpriteAnimator, animated_sprite},
    camera::CameraPos,
    collision::{self, Collider, Contacts},
    coords::{Velocity, WorldPos, px, z},
    data::{JsonAsset, JsonLoader, RoomDef, SpriteSheet, attr},
    hud_message::HudMessages,
    input::Joypads,
    objects::{Obj, combat::{HitBox, frame_boxes}, flag},
    player::{Action, CharacterClass, Player, PlayerState, Stats},
    room::{CurrentRoom, RoomEntity},
};
use crate::{GameClock, GameSystems, controller::Button};
use bevy::prelude::*;
use super::audio::{PlaySound, SetMusicTimer, level_up_jingle};
use serde::Deserialize;

/// The cartridge's Explosion hits during its launch delay with the damage left over from the
/// previous spell (docs/re/magic.md §3.5); set to false to deal the intended 100 instead.
pub const EXPLOSION_STALE_DAMAGE_QUIRK: bool = true;

const COST: [i32; 6] = [1, 2, 4, 8, 20, 80];
const DAMAGE: [i32; 6] = [20, 40, 20, 40, 20, 100];
const NAMES: [&str; 5] = ["basic_magic", "flying_dagger", "flame_wall", "blizzard", "lightning_bolt"];

/// Spell tables (`data/magic.data.json`).
#[derive(Asset, TypePath, Deserialize, Debug)]
pub struct MagicDef {
    pub spells: Vec<serde_json::Value>,
}

impl JsonAsset for MagicDef {}

#[derive(Resource)]
pub struct MagicAssets {
    pub def: Handle<MagicDef>,
    pub icon: Handle<SpriteSheet>,
    pub sheets: Vec<Handle<SpriteSheet>>,
}

/// The Mage's spell state (one spell at a time; cartridge 0xFF0B58..0xFF0B86).
#[derive(Resource, Default, Debug)]
pub struct Magic {
    charge: u8,
    counter: u32,
    /// Selected spell (player +0xA0); persists between casts.
    pub selected: usize,
    pub active: bool,
    spell: usize,
    stage: u8,
    delay: u32,
    timer: u32,
    /// Damage of the last launched spell (slot 0 +102) – used by the Explosion quirk.
    last_damage: i32,
    explosion_done: bool,
    // Flame wall.
    fire_last: IVec2,
    fire_step: i32,
    fire_blocked: bool,
    fire_spawned: u32,
    fire_countdown: u32,
    // Blizzard.
    ring: usize,
    centre: IVec2,
    half: u32,
    facing: i32,
    // Lightning.
    hist: [[usize; 8]; 2],
}

impl Magic {
    /// Room 12's forest loop (0x3138A): while a spell exists (0xFF0B66), the Flame Wall's last flame
    /// x (0xFF0B6A, spell 2) or the Blizzard centre x (0xFF0B7A, spell 3) moves with the camera.
    pub fn shift_x(&mut self, dx: i32) {
        if !self.active {
            return;
        }
        match self.spell {
            2 => self.fire_last.x += dx,
            3 => self.centre.x += dx,
            _ => {}
        }
    }
}

/// One spell projectile (cartridge spell object slot).
#[derive(Component, Debug)]
pub struct SpellProj {
    pub slot: usize,
    pub damage: i32,
    pub alive: bool,
    toggle: u32,
    angle: usize,
}

#[derive(Component)]
struct SpellIcon;

fn load(mut commands: Commands, server: Res<AssetServer>) {
    commands.insert_resource(MagicAssets {
        def: server.load("data/magic.magic.json"),
        icon: server.load("sprites/magic/icon.sheet.json"),
        sheets: NAMES.iter().map(|n| server.load(format!("sprites/magic/{n}.sheet.json"))).collect(),
    });
}

fn spawn_point(p: &Player, pos: &WorldPos) -> IVec2 {
    let f = p.facing.sign();
    let d = match p.state {
        PlayerState::ClimbIdle | PlayerState::ClimbMove => 26,
        PlayerState::Crouch => 18,
        _ => 36,
    };
    IVec2::new(pos.px_x() + 48 * f, pos.px_y() - d)
}

/// Charging and casting (cartridge 0xA9F0).
#[allow(clippy::too_many_arguments)]
fn charge(
    mut commands: Commands,
    pads: Res<Joypads>,
    assets: Res<MagicAssets>,
    mut magic: ResMut<Magic>,
    mut hud: ResMut<HudMessages>,
    mut sound: MessageWriter<PlaySound>,
    mut players: Query<(&Player, &CharacterClass, &mut Stats, &WorldPos)>,
    mut icon: Query<(Entity, &mut WorldPos, &mut SpriteAnimator), (With<SpellIcon>, Without<Player>)>,
) {
    let Some((p, class, mut stats, pos)) = players.iter_mut().find(|(_, c, ..)| **c == CharacterClass::Mage) else { return };
    let pad = pads.get(p.slot);
    let cancel = |m: &mut Magic, commands: &mut Commands, icon: &Query<(Entity, &mut WorldPos, &mut SpriteAnimator), (With<SpellIcon>, Without<Player>)>| {
        m.charge = 0;
        m.counter = 0;
        for (e, ..) in icon {
            commands.entity(e).despawn();
        }
    };
    if *class != CharacterClass::Mage {
        return;
    }
    if p.state >= PlayerState::Hurt {
        cancel(&mut magic, &mut commands, &icon);
        return;
    }
    if magic.active {
        return;
    }
    let icon_pos = WorldPos::from_px(pos.px_x() + 8, pos.px_y() - 56);
    let show = |m: &Magic, commands: &mut Commands, icon: &mut Query<(Entity, &mut WorldPos, &mut SpriteAnimator), (With<SpellIcon>, Without<Player>)>| {
        let name = format!("spell_{}", m.selected);
        if let Ok((_, mut ip, mut a)) = icon.single_mut() {
            *ip = icon_pos;
            a.restart(&name);
        } else {
            commands.spawn((SpellIcon, icon_pos, animated_sprite(SpriteAnimator::new(assets.icon.clone(), name, z::PLAYER + 1.0)), RoomEntity));
        }
    };
    if magic.charge == 0 {
        if !pad.pressed(Button::B) {
            magic.counter = 0;
            return;
        }
        magic.charge = 1;
    }
    if magic.charge == 1 {
        if pad.released(Button::B) {
            cancel(&mut magic, &mut commands, &icon);
            return;
        }
        magic.counter += 1;
        if magic.counter < 30 {
            return;
        }
        magic.counter = 0;
        magic.charge = 2;
        show(&magic, &mut commands, &mut icon);
        sound.write(PlaySound(0xB3));
        return;
    }
    // Icon shown: release casts, holding cycles.
    if pad.released(Button::B) {
        let cost = COST[magic.selected];
        if stats.mp < cost {
            hud.push_to(p.slot, 5);
        } else {
            stats.mp -= cost;
            magic.active = true;
            magic.spell = magic.selected;
            magic.stage = 0;
        }
        cancel(&mut magic, &mut commands, &icon);
        return;
    }
    if let Ok((_, mut ip, _)) = icon.single_mut() {
        *ip = icon_pos;
    }
    magic.counter += 1;
    if magic.counter < 30 {
        return;
    }
    magic.counter = 0;
    magic.selected = if magic.selected >= stats.spells_known as usize { 0 } else { magic.selected + 1 };
    show(&magic, &mut commands, &mut icon);
    sound.write(PlaySound(0xB3));
}

fn spawn_proj(commands: &mut Commands, sheet: &Handle<SpriteSheet>, anim: &str, slot: usize, damage: i32, at: IVec2, vel: Velocity) -> Entity {
    let mut a = SpriteAnimator::new(sheet.clone(), anim, z::PLAYER + 0.5);
    a.manual = true;
    commands
        .spawn((
            SpellProj { slot, damage, alive: true, toggle: 0, angle: 0 },
            WorldPos::from_px(at.x, at.y),
            vel,
            animated_sprite(a),
            RoomEntity,
        ))
        .id()
}

/// Flame wall floor search (cartridge 0xCAFE).
fn floor_search(room: &RoomDef, x: i32, mut y: i32) -> Option<i32> {
    let mut push = 0;
    let mut tries = 0;
    loop {
        let mut retry = false;
        for px_ in [x - 8, x + 7] {
            let a = room.attr_at(px_, y);
            if attr::is_solid(a) {
                y &= !7;
                if attr::is_solid(room.attr_at(x, y - 1)) {
                    push += 1;
                    if push >= 5 {
                        return None;
                    }
                    y -= 8;
                    retry = true;
                    break;
                }
                return Some(y);
            }
            if a & 0xF0 == 0x10 {
                return Some(y);
            }
        }
        if retry {
            continue;
        }
        y += 16;
        tries += 1;
        if tries >= 3 {
            return None;
        }
    }
}

fn on_screen(cam: &CameraPos, x: i32) -> bool {
    let sx = x - cam.x;
    (-16..336).contains(&sx)
}

/// Cast stages, launch and per-spell updates (cartridge 0xABF4).
#[allow(clippy::too_many_arguments)]
fn run_spell(
    mut commands: Commands,
    assets: Res<MagicAssets>,
    defs: Res<Assets<MagicDef>>,
    sheets: Res<Assets<SpriteSheet>>,
    mut magic: ResMut<Magic>,
    mut sound: MessageWriter<PlaySound>,
    cam: Res<CameraPos>,
    room: Option<Res<CurrentRoom>>,
    rooms: Res<Assets<RoomDef>>,
    mut players: Query<(&mut Player, &mut Velocity, &Stats, &WorldPos, &CharacterClass), Without<SpellProj>>,
    objs: Query<(&Obj, &WorldPos), (Without<SpellProj>, Without<Player>)>,
    mut projs: Query<
        (Entity, &mut SpellProj, &mut WorldPos, &mut Velocity, &mut SpriteAnimator, &mut Visibility, Option<&mut BlizzardAngle>),
        Without<Player>,
    >,
) {
    if !magic.active {
        return;
    }
    let (Some(def), Some(room)) = (defs.get(&assets.def), room.and_then(|r| rooms.get(&r.def).map(|d| (r.index, d)))) else { return };
    let (room_index, room_def) = room;
    let Some((mut p, mut pvel, stats, ppos, _)) = players.iter_mut().find(|(.., c)| **c == CharacterClass::Mage) else { return };
    let f = p.facing.sign();
    let caster = IVec2::new(ppos.px_x(), ppos.px_y());
    match magic.stage {
        0 => {
            // The caster swings as for a normal attack; walking stops.
            magic.stage = 1;
            magic.delay = 10;
            if p.state == PlayerState::Walk {
                pvel.x = 0;
                pvel.y = 0;
                p.state = PlayerState::Stand;
            }
            p.action = Action::Attack;
            p.power = stats.atk;
            p.swing_id = p.swing_id.wrapping_add(1);
            magic.explosion_done = false;
            return;
        }
        1 => {
            magic.delay -= 1;
            if magic.delay > 0 {
                return;
            }
            magic.stage = 2;
            magic.timer = 0;
            let sp = spawn_point(&p, ppos);
            let spell = magic.spell;
            if spell < 5 {
                magic.last_damage = DAMAGE[spell];
            }
            let sheet = assets.sheets.get(spell).cloned();
            // Launch sound (A6; Lightning AC, Explosion 9B).
            if let Some(id) = def.spells[spell]["sound"].as_u64() {
                sound.write(PlaySound(id as u32));
            }
            match spell {
                0 => {
                    let anim = if f > 0 { "right" } else { "left" };
                    spawn_proj(&mut commands, sheet.as_ref().unwrap(), anim, 0, 20, sp, Velocity { x: px(6) * f, y: 0 });
                }
                1 => {
                    let table = def.spells[1]["velocity_table_0xB310"].as_array().cloned().unwrap_or_default();
                    for k in 0..8 {
                        let v = &table[7 - k];
                        let vel = Velocity { x: (v[0].as_f64().unwrap_or(0.0) * 65536.0) as i32, y: (v[1].as_f64().unwrap_or(0.0) * 65536.0) as i32 };
                        spawn_proj(&mut commands, sheet.as_ref().unwrap(), "spin", k, 40, sp, vel);
                    }
                }
                2 => {
                    magic.fire_spawned = 0;
                    magic.fire_blocked = false;
                    if !(p_on_ground(&p)) {
                        magic.active = false;
                        return;
                    }
                    magic.fire_step = 16 * f;
                    let x = ((caster.x + 24 * f) & !7) | 8;
                    let y = caster.y + 1;
                    match floor_search(room_def, x, y).filter(|_| on_screen(&cam, x)) {
                        Some(y) => {
                            spawn_proj(&mut commands, sheet.as_ref().unwrap(), "burn", 0, 20, IVec2::new(x, y), Velocity::default());
                            magic.fire_last = IVec2::new(x, y);
                            magic.fire_spawned = 1;
                            magic.fire_countdown = 8;
                        }
                        None => magic.active = false,
                    }
                }
                3 => {
                    magic.ring = 0;
                    magic.half = 0;
                    magic.centre = sp;
                    magic.facing = f;
                    for k in 0..8 {
                        let e = spawn_proj(&mut commands, sheet.as_ref().unwrap(), &format!("angle_{}", 4 * (7 - k)), k, 40, sp, Velocity::default());
                        commands.entity(e).insert(BlizzardAngle(4 * (7 - k)));
                    }
                }
                4 => {
                    let d = if f > 0 { 1 } else { 7 };
                    magic.hist = [[4; 8]; 2];
                    magic.hist[0][0] = d;
                    magic.hist[1][0] = d;
                    for k in 0..16 {
                        spawn_proj(&mut commands, sheet.as_ref().unwrap(), &format!("dir_{d}"), k, 20, sp, Velocity::default());
                    }
                }
                _ => {
                    // Explosion: no object, nothing drawn; the hit test handles it.
                    magic.timer = 0;
                }
            }
            return;
        }
        _ => {}
    }
    // Stage 2: per-spell update.
    magic.timer += 1;
    let t = magic.timer;
    let spell = magic.spell;
    let mut any_alive = false;
    match spell {
        0 => {
            for (e, mut s, mut pos, mut vel, mut a, _, _) in &mut projs {
                let mut end = !s.alive;
                if room_index == 24 {
                    pos.x += vel.x;
                } else {
                    let mut c = Contacts::default();
                    collision::horizontal_only(room_def, &Collider { half_width: 8, height: 16 }, &mut pos, &vel, &mut c);
                    end |= c.wall_left || c.wall_right;
                }
                end |= !on_screen(&cam, pos.px_x());
                if end {
                    s.alive = false;
                    commands.entity(e).despawn();
                } else {
                    any_alive = true;
                    if let Some(sh) = sheets.get(&a.sheet) {
                        anim::step(sh, &mut a);
                    }
                }
                let _ = &mut vel;
            }
        }
        1 => {
            for (e, mut s, mut pos, mut vel, mut a, mut vis, _) in &mut projs {
                s.toggle += 1;
                if s.toggle == 2 {
                    s.toggle = 0;
                    let (tx, ty) = (caster.x, caster.y - 32);
                    let step = 0x7000;
                    vel.x = if tx >= pos.px_x() { (vel.x + step).min(px(7)) } else { (vel.x - step).max(-px(7)) };
                    vel.y = if ty >= pos.px_y() { (vel.y + step).min(px(7)) } else { (vel.y - step).max(-px(7)) };
                }
                pos.x += vel.x;
                pos.y += vel.y;
                if let Some(sh) = sheets.get(&a.sheet) {
                    anim::step(sh, &mut a);
                }
                *vis = if s.alive { Visibility::Inherited } else { Visibility::Hidden };
                if t >= 180 {
                    commands.entity(e).despawn();
                } else {
                    any_alive = true;
                }
            }
        }
        2 => {
            let mut live = 0;
            let mut used = vec![];
            for (e, s, _, _, mut a, _, _) in &mut projs {
                if let Some(sh) = sheets.get(&a.sheet) {
                    anim::step(sh, &mut a);
                }
                if a.cycled {
                    commands.entity(e).despawn();
                } else {
                    live += 1;
                    used.push(s.slot);
                }
            }
            if magic.fire_spawned != 15 && !magic.fire_blocked {
                magic.fire_countdown -= 1;
                if magic.fire_countdown == 0 {
                    magic.fire_countdown = 8;
                    if let Some(slot) = (0..8).find(|s| !used.contains(s)) {
                        let x = magic.fire_last.x + magic.fire_step;
                        match floor_search(room_def, x, magic.fire_last.y).filter(|_| on_screen(&cam, x)) {
                            Some(y) => {
                                spawn_proj(&mut commands, &assets.sheets[2], "burn", slot, 20, IVec2::new(x, y), Velocity::default());
                                magic.fire_last = IVec2::new(x, y);
                                magic.fire_spawned += 1;
                                live += 1;
                            }
                            None => magic.fire_blocked = true,
                        }
                    }
                }
            }
            any_alive = live > 0;
        }
        3 => {
            let rings = &def.spells[3]["ring_table_0xBA2E"];
            magic.half += 1;
            if magic.half >= 2 {
                magic.half = 0;
                for (_, _, mut pos, _, mut a, _, ang) in &mut projs {
                    if let Some(mut ang) = ang {
                        let d = &rings[magic.ring][ang.0];
                        let (dx, dy) = (d[0].as_i64().unwrap_or(0) as i32, d[1].as_i64().unwrap_or(0) as i32);
                        *pos = WorldPos::from_px(magic.centre.x + dx, magic.centre.y + dy);
                        a.swap_keep_frame(&format!("angle_{}", ang.0));
                        ang.0 = (ang.0 + 31) & 31;
                    }
                }
                magic.centre.x += 2 * magic.facing;
                if magic.ring != 29 {
                    magic.ring += 1;
                }
            }
            for (e, ..) in &mut projs {
                if t >= 180 {
                    commands.entity(e).despawn();
                } else {
                    any_alive = true;
                }
            }
        }
        4 => {
            // Steer both chains towards the first enemies (chain B prefers a different one).
            let mut picked: Option<IVec2> = None;
            let mut heads = [IVec2::ZERO; 2];
            for (_, s, pos, ..) in &projs {
                if s.slot == 0 || s.slot == 8 {
                    heads[s.slot / 8] = IVec2::new(pos.px_x(), pos.px_y());
                }
            }
            let mut targets: Vec<IVec2> = objs
                .iter()
                .filter(|(o, _)| o.slot < 8 && !o.delete && o.flags & flag::UNHITTABLE == 0)
                .map(|(o, p)| (o.slot, IVec2::new(p.px_x(), p.px_y())))
                .collect::<Vec<_>>()
                .into_iter()
                .map(|(_, p)| p)
                .collect();
            for chain in 0..2 {
                let head = heads[chain];
                let target = targets.iter().copied().find(|t| Some(*t) != picked).inspect(|t| picked = Some(*t)).unwrap_or(caster);
                let xc = if target.x >= head.x + 4 { 1 } else if target.x < head.x - 4 { 7 } else { 4 };
                let yc = if target.y < head.y - 4 { 0 } else if target.y >= head.y + 4 { 2 } else { 1 };
                let raw = (xc + yc - 1) as usize;
                let turn = &def.spells[4]["turn_table_0xCC4A"];
                let cur = magic.hist[chain][0];
                let new = turn[cur][raw].as_u64().unwrap_or(cur as u64) as usize;
                magic.hist[chain].rotate_right(1);
                magic.hist[chain][0] = new;
            }
            targets.clear();
            let dirs = def.spells[4]["dir_velocity_0xB5CA"].as_array().cloned().unwrap_or_default();
            for (e, s, mut pos, _, mut a, mut vis, _) in &mut projs {
                let chain = s.slot / 8;
                let d = magic.hist[chain][s.slot % 8];
                if d != 4 {
                    let v = &dirs[d];
                    pos.x += px(v[0].as_i64().unwrap_or(0) as i32);
                    pos.y += px(v[1].as_i64().unwrap_or(0) as i32);
                    a.swap_keep_frame(&format!("dir_{d}"));
                }
                if let Some(sh) = sheets.get(&a.sheet) {
                    anim::step(sh, &mut a);
                }
                // Only one chain is drawn per tick, alternating.
                let drawn = if t % 2 == 1 { chain == 0 } else { chain == 1 };
                *vis = if drawn && s.alive { Visibility::Inherited } else { Visibility::Hidden };
                if t >= 300 {
                    commands.entity(e).despawn();
                } else {
                    any_alive = true;
                }
            }
        }
        _ => {
            any_alive = t < 1;
        }
    }
    if !any_alive {
        magic.active = false;
    }
}

/// Blizzard shard orbit angle.
#[derive(Component)]
struct BlizzardAngle(usize);

fn p_on_ground(p: &Player) -> bool {
    matches!(p.state, PlayerState::Stand | PlayerState::Walk | PlayerState::Crouch)
}

/// Attack box of a spell object (+54..+60).
fn proj_box(spell: usize, sheets: &Assets<SpriteSheet>, sanim: &SpriteAnimator, spos: &WorldPos) -> Option<HitBox> {
    if spell == 3 {
        // Blizzard shards never get hit boxes: a point test at the shard (quirk, §3.3).
        Some(HitBox { cx: spos.px_x(), cy: spos.px_y(), w: 0, h: 0 })
    } else {
        sheets.get(&sanim.sheet).and_then(|sh| frame_boxes(sh, sanim, spos).1)
    }
}

/// What object scripts see of the Mage's spell this tick: the spell-active flag (0xFF0B66), the
/// spell (0xFF0B60) and the live spell objects' attack boxes in slot order (0xFFB824 + k·0x100).
/// Read by the hidden-item spot's C124 (docs/re/hidden_items.md).
#[derive(Resource, Default, Debug, Clone)]
pub struct SpellView {
    pub active: bool,
    pub spell: usize,
    pub boxes: Vec<(usize, Option<HitBox>)>,
}

fn publish_view(
    magic: Res<Magic>,
    sheets: Res<Assets<SpriteSheet>>,
    projs: Query<(&SpellProj, &WorldPos, &SpriteAnimator)>,
    mut view: ResMut<SpellView>,
) {
    view.active = magic.active;
    view.spell = magic.spell;
    view.boxes.clear();
    if magic.active {
        view.boxes.extend(projs.iter().filter(|(s, ..)| s.alive).map(|(s, p, a)| (s.slot, proj_box(magic.spell, &sheets, a, p))));
        view.boxes.sort_by_key(|b| b.0);
    }
}

/// Spell projectiles hitting objects (cartridge 0x14662, odd ticks).
#[allow(clippy::too_many_arguments)]
fn spell_hits(
    clock: Res<GameClock>,
    mut magic: ResMut<Magic>,
    sheets: Res<Assets<SpriteSheet>>,
    data: GameDataRef,
    mut hud: ResMut<HudMessages>,
    mut sound: MessageWriter<PlaySound>,
    mut timer: MessageWriter<SetMusicTimer>,
    mut players: Query<(&Player, &CharacterClass, &mut Stats)>,
    mut projs: Query<(&mut SpellProj, &WorldPos, &SpriteAnimator)>,
    mut objs: Query<(&mut Obj, &WorldPos, &SpriteAnimator), Without<SpellProj>>,
) {
    if clock.tick % 2 == 0 {
        return;
    }
    // Kills by spells count for the Mage (projectile +106 = caster).
    let Some((caster, class, mut stats)) = players.iter_mut().find(|(_, c, _)| **c == CharacterClass::Mage) else { return };
    let slot = caster.slot;
    let mut boss_damage = 0;
    for (mut o, opos, oanim) in &mut objs {
        if o.delete || o.flags & flag::UNHITTABLE != 0 {
            continue;
        }
        if o.invuln != 0 {
            o.invuln -= 1;
            continue;
        }
        if !magic.active {
            continue;
        }
        let Some(osheet) = sheets.get(&oanim.sheet) else { continue };
        let Some(body) = super::objects::ops::object_boxes(&o, osheet, oanim, opos).0 else { continue };
        let mut damage = None;
        if magic.spell == 5 {
            if magic.explosion_done {
                continue;
            }
            damage = Some(if EXPLOSION_STALE_DAMAGE_QUIRK && magic.stage < 2 { magic.last_damage } else { 100 });
        } else {
            for (mut s, spos, sanim) in &mut projs {
                if !s.alive || s.damage == 0 {
                    continue;
                }
                let atk = proj_box(magic.spell, &sheets, sanim, spos);
                if atk.is_some_and(|a| a.overlaps(&body)) {
                    if magic.spell <= 1 {
                        s.alive = false;
                    }
                    damage = Some(s.damage);
                    break;
                }
            }
        }
        let Some(dmg) = damage else { continue };
        // Bounce (A5), hit (A7), kill (A8): cartridge 0x14714.
        if o.flags & flag::MAGIC_PROOF != 0 {
            sound.write(PlaySound(0xA5));
            o.invuln = 16;
            continue;
        }
        sound.write(PlaySound(0xA7));
        // Boss parts pass the damage to the boss in slot 0 (cartridge 0x14758).
        if o.flags & flag::BOSS_PART != 0 {
            boss_damage += dmg;
            continue;
        }
        if o.hp > dmg {
            o.hp -= dmg;
            o.hit_count += 1;
            o.flash = 1;
            o.invuln = 16;
        } else if dmg > 0 {
            sound.write(PlaySound(0xA8));
            o.hp = 0;
            o.pending_death = true;
            o.flags |= flag::UNHITTABLE | flag::HARMLESS;
            stats.exp = stats.exp.saturating_add(o.exp as u32).min(65535);
            if let Some(d) = data.get() {
                let spells_before = stats.spells_known;
                if super::player::check_level_up(&mut stats, *class, d) {
                    hud.push_to(slot, super::lifecycle::msg::LEVEL_UP);
                        level_up_jingle(&mut sound, &mut timer);
                    if *class == CharacterClass::Mage && stats.spells_known > spells_before {
                        hud.push_to(slot, 5 + stats.spells_known);
                    }
                }
            }
        } else {
            o.invuln = 16;
        }
    }
    if boss_damage > 0 {
        if let Some((mut o, ..)) = objs.iter_mut().find(|(o, ..)| o.slot == 0 && !o.delete) {
            o.hp -= boss_damage;
            o.hit_count += 1;
            o.flash = 1;
            o.invuln = 16;
            if o.hp <= 0 {
                o.hp = 0;
                o.pending_death = true;
                o.flags |= flag::UNHITTABLE | flag::HARMLESS;
                stats.exp = stats.exp.saturating_add(o.exp as u32).min(65535);
                if let Some(d) = data.get() {
                    if super::player::check_level_up(&mut stats, *class, d) {
                        hud.push_to(slot, super::lifecycle::msg::LEVEL_UP);
                        level_up_jingle(&mut sound, &mut timer);
                    }
                }
            }
        }
    }
    if magic.spell == 5 && magic.stage == 2 {
        magic.explosion_done = true;
    }
}

/// Fireball of the tier-2 weapons (Blade of Fire, Staff of Fire): cartridge 0xB692 / 0xB7B4.
#[derive(Component)]
pub struct WeaponShot {
    alive: bool,
    /// Slot of the player who fired it (gets the EXP; one shot per player at a time).
    owner: usize,
}

/// Per-player weapon-shot state (+172 cooldown, +174 already fired this swing).
#[derive(Default, Clone, Copy)]
struct Shooter {
    cooldown: u32,
    fired: bool,
}

#[derive(Resource, Default)]
struct ShotState {
    players: [Shooter; crate::controller::PORTS],
}

#[allow(clippy::too_many_arguments)]
fn weapon_shots(
    mut commands: Commands,
    server: Res<AssetServer>,
    mut st: ResMut<ShotState>,
    magic: Res<Magic>,
    cam: Res<CameraPos>,
    room: Option<Res<CurrentRoom>>,
    rooms: Res<Assets<RoomDef>>,
    sheets: Res<Assets<SpriteSheet>>,
    players: Query<(&Player, &CharacterClass, &super::player_look::Gear, &WorldPos, &SpriteAnimator), Without<WeaponShot>>,
    mut shots: Query<(Entity, &mut WeaponShot, &mut WorldPos, &Velocity, &mut SpriteAnimator), Without<Player>>,
) {
    let Some((room_index, def)) = room.and_then(|r| rooms.get(&r.def).map(|d| (r.index, d))) else { return };
    // Move existing shots.
    for (e, s, mut pos, vel, mut a) in &mut shots {
        let mut end = !s.alive;
        if room_index == 24 {
            pos.x += vel.x;
        } else {
            let mut c = Contacts::default();
            collision::horizontal_only(def, &Collider { half_width: 8, height: 16 }, &mut pos, vel, &mut c);
            end |= c.wall_left || c.wall_right;
        }
        end |= !on_screen(&cam, pos.px_x());
        if end {
            commands.entity(e).despawn();
        } else if let Some(sh) = sheets.get(&a.sheet) {
            anim::step(sh, &mut a);
        }
    }
    let owners: Vec<usize> = shots.iter().map(|(_, s, ..)| s.owner).collect();
    for (p, class, gear, pos, anim) in &players {
        let Some(st) = st.players.get_mut(p.slot) else { continue };
        fire_weapon_shot(&mut commands, &server, st, &magic, p, class, gear, pos, anim, owners.contains(&p.slot));
    }
}

#[allow(clippy::too_many_arguments)]
fn fire_weapon_shot(
    commands: &mut Commands,
    server: &AssetServer,
    st: &mut Shooter,
    magic: &Magic,
    p: &Player,
    class: &CharacterClass,
    gear: &super::player_look::Gear,
    pos: &WorldPos,
    anim: &SpriteAnimator,
    shot_out: bool,
) {
    if st.cooldown > 0 {
        st.cooldown -= 1;
        return;
    }
    let weapon = if *class == CharacterClass::Fighter { gear.ac6 } else { gear.aca };
    let strike = match p.action {
        Action::Attack => anim.index == 1,
        Action::UpAttack => anim.index == 2,
        _ => false,
    };
    if !strike {
        st.fired = false;
        return;
    }
    // A spell's swing does not shoot; one shot per swing; only one shot on screen.
    let casting = *class == CharacterClass::Mage && magic.active;
    if weapon != 2 || st.fired || casting || shot_out {
        return;
    }
    st.fired = true;
    st.cooldown = 0x11;
    let at = spawn_point(p, pos);
    let f = p.facing.sign();
    let mut a = SpriteAnimator::new(server.load("sprites/magic/weapon_shot.sheet.json"), if f > 0 { "right" } else { "left" }, z::PLAYER + 0.5);
    a.manual = true;
    commands.spawn((WeaponShot { alive: true, owner: p.slot }, WorldPos::from_px(at.x, at.y), Velocity { x: px(6) * f, y: 0 }, animated_sprite(a), RoomEntity));
}

/// Weapon shots hitting objects: 8 damage, ignores defence, consumed (cartridge 0x14848).
#[allow(clippy::too_many_arguments)]
fn weapon_shot_hits(
    clock: Res<GameClock>,
    sheets: Res<Assets<SpriteSheet>>,
    data: GameDataRef,
    mut hud: ResMut<HudMessages>,
    mut sound: MessageWriter<PlaySound>,
    mut timer: MessageWriter<SetMusicTimer>,
    mut players: Query<(&Player, &CharacterClass, &mut Stats)>,
    mut shots: Query<(&mut WeaponShot, &WorldPos, &SpriteAnimator)>,
    mut objs: Query<(&mut Obj, &WorldPos, &SpriteAnimator), Without<WeaponShot>>,
) {
    if clock.tick % 2 == 0 {
        return;
    }
    for (mut s, spos, sanim) in &mut shots {
        let Some((_, class, mut stats)) = players.iter_mut().find(|(p, ..)| p.slot == s.owner) else { continue };
        let slot = s.owner;
        let Some(atk) = sheets.get(&sanim.sheet).and_then(|sh| frame_boxes(sh, sanim, spos).1) else { continue };
        for (mut o, opos, oanim) in &mut objs {
            if !s.alive || o.delete || o.flags & flag::UNHITTABLE != 0 {
                continue;
            }
            let Some(osheet) = sheets.get(&oanim.sheet) else { continue };
            let Some(body) = super::objects::ops::object_boxes(&o, osheet, oanim, opos).0 else { continue };
            if !atk.overlaps(&body) {
                continue;
            }
            s.alive = false;
            if o.flags & flag::MAGIC_PROOF != 0 {
                continue;
            }
            if o.hp > 8 {
                o.hp -= 8;
                o.hit_count += 1;
                o.flash = 1;
            } else {
                o.hp = 0;
                o.pending_death = true;
                o.flags |= flag::UNHITTABLE | flag::HARMLESS;
                stats.exp = stats.exp.saturating_add(o.exp as u32).min(65535);
                if let Some(d) = data.get() {
                    if super::player::check_level_up(&mut stats, *class, d) {
                        hud.push_to(slot, super::lifecycle::msg::LEVEL_UP);
                        level_up_jingle(&mut sound, &mut timer);
                    }
                }
            }
        }
    }
}

pub fn plugin(app: &mut App) {
    app.init_asset::<MagicDef>()
        .register_asset_loader(JsonLoader::<MagicDef>::new(&["magic.json"]))
        .init_resource::<Magic>()
        .init_resource::<ShotState>()
        .init_resource::<SpellView>()
        .add_systems(Startup, load)
        .add_systems(
            FixedUpdate,
            (charge, run_spell, spell_hits, publish_view, weapon_shots, weapon_shot_hits)
                .chain()
                .in_set(GameSystems::Game)
                .after(super::player::tick)
                // Cartridge frame order: spell stage machine before the object loop (0x4114).
                .before(super::objects::tick_objects)
                .run_if(in_state(AppState::InGame))
                .run_if(super::objects::objects_running),
        );
}
