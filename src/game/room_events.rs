//! Scripted per-room events (docs/re/room_events.md).
//!
//! The cartridge runs a per-room dispatcher every frame (0x31082, skipped while a window pauses the
//! game), a room-entry hook (0x30EC6) and a vblank hook (0x3183A). Their parameters are extracted
//! into `data/room_events.events.json` by `extract/room_events.py`; this module plays them back:
//!
//! * room 2: plane-B background swaps (three nametables, scroll registers, freeze flags, palette
//!   cycle sets) keyed on the camera block, and an "earthquake" vertical shake in the lava area;
//! * room 4 / 27: a jingle once a story flag is set;
//! * room 9, 44, 46, 47: background / camera freeze, room 9's entry message;
//! * room 12: the endlessly looping forest (moving left at camera block 10 jumps 10 blocks right)
//!   and the Lilliput Orchid shrink/grow tiles;
//! * room 13: the Silk of the Worm thread that becomes a rope;
//! * room 19: the Magic Flute flame bridge, Abel's grave and the rain band;
//! * room 20: the pushable wall; room 22: the player draws above high-priority tiles;
//! * room 47: the ending's second script step.
//!
//! Room attributes changed by an event are restored when the room is left (the cartridge reloads
//! them on every room entry), story flags persist in [`StoryFlags`].
use super::{
    AppState,
    anim::SpriteAnimator,
    camera::{self, CameraPos},
    collision::Contacts,
    coords::{Velocity, WorldPos, px, to_bevy, z},
    data::{JsonAsset, JsonLoader, PaletteCycle, RoomDef},
    dialog::{Dialog, StoryFlags},
    input::DemoInput,
    magic::{Magic, SpellProj, WeaponShot},
    objects::ops::Effects,
    player::{Action, Player, PlayerState},
    room::{BackgroundLayer, BackgroundTile, CurrentRoom, CycleMask, CycleState, ForegroundLayer, RoomEntity},
    ui::ScreenRoot,
};
use crate::{GameClock, GameSystems};
use bevy::{asset::LoadContext, prelude::*, sprite::Anchor};
use serde::Deserialize;
use std::collections::HashMap;

pub const EVENTS_PATH: &str = "data/room_events.events.json";

// ---------------------------------------------------------------------------
// Data
// ---------------------------------------------------------------------------

/// A plane-B nametable variant with its scroll registers and palette cycles.
#[derive(Deserialize, Debug, Clone)]
pub struct BgVariant {
    pub image: String,
    /// Value written to the plane-B hscroll register (screen x shows background x - hscroll).
    pub hscroll: i32,
    /// Value written to the plane-B vscroll register (screen y shows background y + vscroll).
    pub vscroll: i32,
    /// 0xFF0B99 bits 0/1: the background stops following the camera on that axis.
    pub freeze: [bool; 2],
    pub cycles: Vec<PaletteCycle>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct BgTrigger {
    /// Camera block y range [min, max).
    pub cam_block_y: [i32; 2],
    pub cam_block_x: Vec<i32>,
    pub variant: String,
    /// "default": only while the default background shows; "other": only while a variant shows.
    pub from: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ShakeDef {
    pub amplitude: i32,
    pub cam_block_y_min: i32,
    /// Camera block x range [min, max).
    pub cam_block_x: [i32; 2],
}

#[derive(Deserialize, Debug, Clone)]
pub struct BgSwapDef {
    pub room: usize,
    pub variants: HashMap<String, BgVariant>,
    pub triggers: Vec<BgTrigger>,
    pub shake: Option<ShakeDef>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct SpriteDef {
    pub image: String,
    pub origin: [i32; 2],
}

#[derive(Deserialize, Debug, Clone)]
pub struct SilkRopeDef {
    pub room: usize,
    pub cam_block_x: i32,
    pub cam_block_y: [i32; 2],
    pub item_flag: String,
    pub message_ok: u32,
    pub message_ok_shape: usize,
    pub message_fail: u32,
    pub message_fail_shape: usize,
    pub sprite: SpriteDef,
    pub start: [i32; 2],
    pub end_y: i32,
    pub step: i32,
    pub rope_cells: Vec<[i32; 2]>,
    pub rope_attr: u8,
}

#[derive(Deserialize, Debug, Clone)]
pub struct BridgeDef {
    pub done_flag: String,
    pub item_flag: String,
    pub cam_block_x: i32,
    pub message_ok: u32,
    pub message_ok_shape: usize,
    pub message_fail: u32,
    pub message_fail_shape: usize,
    pub sound: u32,
    pub wait_frames: u32,
    pub block_every: u64,
    pub blocks_drawn: i32,
    pub row: i32,
    pub col0: i32,
    pub cells: i32,
    pub open_attr: u8,
    pub closed_attr: u8,
    pub wall_cells: Vec<[i32; 2]>,
    pub block_image: String,
    pub fg_closed: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct RainDef {
    pub image: String,
    pub origin: [i32; 2],
    pub x: i32,
    pub y0: i32,
    pub y_end: i32,
    pub step: i32,
    pub on_right: i32,
    pub off_right: i32,
    pub on_left: i32,
    pub off_left: i32,
}

#[derive(Deserialize, Debug, Clone)]
pub struct GraveDef {
    pub attr: u8,
    pub done_flag: String,
    pub item_flag: String,
    pub message_read: u32,
    pub message_read_shape: usize,
    pub message_charm: u32,
    pub message_charm_shape: usize,
}

#[derive(Deserialize, Debug, Clone)]
pub struct GraveyardDef {
    pub room: usize,
    pub bridge: BridgeDef,
    pub rain: RainDef,
    pub grave: GraveDef,
}

#[derive(Deserialize, Debug, Clone)]
pub struct WallUnlockDef {
    pub room: usize,
    pub flag: String,
    pub done_flag: String,
    pub cells: Vec<[i32; 2]>,
    pub attr: u8,
}

#[derive(Deserialize, Debug, Clone)]
pub struct JingleDef {
    pub room: usize,
    pub flag: String,
    pub done_flag: String,
    pub sound: u32,
    pub music_timer: i32,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ForestLoopDef {
    pub room: usize,
    pub cam_block_x: i32,
    pub shift_blocks: i32,
}

#[derive(Deserialize, Debug, Clone)]
pub struct GnomeDef {
    pub room: usize,
    pub item_flag: String,
    pub shrink_attr: u8,
    pub grow_attr: u8,
    pub message: u32,
    pub message_shape: usize,
    pub sound: u32,
}

#[derive(Deserialize, Debug, Clone)]
pub struct FreezeDef {
    pub room: usize,
    /// 0xFF0B99: bit 0/1 freeze plane-B x/y, bit 2/3 freeze the camera x/y.
    pub bits: u8,
}

#[derive(Deserialize, Debug, Clone)]
pub struct EntryMessageDef {
    pub room: usize,
    pub message: u32,
    pub shape: usize,
    pub unless_flag: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct EndingRoomDef {
    pub room: usize,
    pub delay: u32,
    pub message: u32,
    pub shape: usize,
    /// Page / end timers of text shown in the ending room.
    pub timed: [u32; 2],
    pub demo: Vec<[u8; 2]>,
    /// Entry hook 0x30FEE: both players' walk speed +0xA2 (16.16, 1.0) …
    #[serde(default)]
    pub walk_speed: Option<i32>,
    /// … and their poison flag +0xA6 cleared (0xC2D6 restores the skin colours).
    #[serde(default)]
    pub clear_poison: bool,
}

/// Everything in `data/room_events.events.json`.
#[derive(Asset, TypePath, Deserialize, Debug)]
pub struct RoomEventsDef {
    pub bg_swaps: Vec<BgSwapDef>,
    pub silk_rope: SilkRopeDef,
    pub graveyard: GraveyardDef,
    pub wall_unlock: Vec<WallUnlockDef>,
    pub flag_jingles: Vec<JingleDef>,
    pub forest_loop: ForestLoopDef,
    pub gnome: GnomeDef,
    pub entry_freeze: Vec<FreezeDef>,
    pub entry_messages: Vec<EntryMessageDef>,
    pub player_priority_rooms: Vec<usize>,
    pub ending_room: EndingRoomDef,
    /// Sprite palette line 2 swaps of rooms 24 / 27 (applied by `sprite_palette`).
    #[serde(default)]
    pub palette_line2_swaps: Vec<super::sprite_palette::Line2SwapDef>,
    /// Every image referenced above, kept resident so swaps never pop in.
    #[serde(skip)]
    pub images: HashMap<String, Handle<Image>>,
}

impl JsonAsset for RoomEventsDef {
    fn resolve(&mut self, ctx: &mut LoadContext) {
        let mut paths: Vec<String> = vec![
            self.silk_rope.sprite.image.clone(),
            self.graveyard.bridge.block_image.clone(),
            self.graveyard.bridge.fg_closed.clone(),
            self.graveyard.rain.image.clone(),
        ];
        for swap in &self.bg_swaps {
            for v in swap.variants.values() {
                paths.push(v.image.clone());
                paths.extend(v.cycles.iter().flat_map(|c| c.masks.values().cloned()));
            }
        }
        for p in paths {
            let h = ctx.load(p.clone());
            self.images.insert(p, h);
        }
    }
}

impl RoomEventsDef {
    fn image(&self, path: &str) -> Handle<Image> {
        self.images.get(path).cloned().unwrap_or_default()
    }
}

#[derive(Resource)]
pub struct RoomEventsHandle(pub Handle<RoomEventsDef>);

fn parse_addr(s: &str) -> u32 {
    u32::from_str_radix(s.trim_start_matches("0x").trim_start_matches("0X"), 16).unwrap_or(0)
}

/// A cartridge word flag: set when either byte is non-zero (text scripts set single bytes, item
/// pickups and object scripts use the word address).
fn word_flag(story: &StoryFlags, fx: &Effects, addr: &str) -> bool {
    let a = parse_addr(addr) & !1;
    story.0.get(&a).copied().unwrap_or(0) != 0
        || story.0.get(&(a + 1)).copied().unwrap_or(0) != 0
        || fx.globals.get(&a).copied().unwrap_or(0) != 0
}

fn set_word_flag(story: &mut StoryFlags, addr: &str) {
    story.0.insert((parse_addr(addr) & !1) + 1, 1);
}

// ---------------------------------------------------------------------------
// Runtime state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SilkPhase {
    /// 0xFF0B9E = 0.
    #[default]
    Idle,
    /// 1: thread descending.
    Descending,
    /// 2: thread down, rope usable.
    Down,
    /// 3: no silk, the cliff message was shown.
    Refused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum BridgePhase {
    /// 0xFF0BA6 = 0.
    #[default]
    Idle,
    /// 1: flute message shown, the tune starts next.
    Flute,
    /// 2: tune playing (countdown), then blocks appear every 4 frames.
    Waiting(u32),
    Building(i32),
    /// 3: no flute (message only) or finished.
    Done,
}

/// Per-visit state of the room scripts (the cartridge keeps it in 0xFF0B40..0xFF0C3F, cleared on
/// every room change).
#[derive(Resource, Default)]
pub struct RoomEventState {
    room: Option<usize>,
    /// Room whose attributes were patched, with the original values to restore.
    patched: Option<AssetId<RoomDef>>,
    attr_backup: Vec<(usize, u8)>,
    /// Camera block x seen by the dispatcher last frame (0xFF0B94; 0 on entry).
    last_block: i32,
    /// Second camera tracker of room 19 (0xFF0BB0).
    last_block_rain: i32,
    prev_cam_x: i32,
    entry_camera: IVec2,
    /// 0xFF0B99 set on entry.
    freeze_bits: u8,
    // Room 2.
    bg_variant: String,
    bg_offset: IVec2,
    /// Next shake step (0xFF0B9A) and the vertical scroll it has added so far.
    shake_step: i32,
    shake: i32,
    // Room 12.
    gnome_last_attr: u8,
    gnome_pending: bool,
    // Room 13.
    silk: SilkPhase,
    silk_y: i32,
    // Room 19.
    bridge: BridgePhase,
    rain_on: bool,
    rain_y: i32,
    grave_last_attr: u8,
    // Room 47.
    ending_timer: Option<u32>,
}

/// Set while a room script holds the whole game (room 19's flame bridge: the cartridge loops inside
/// the dispatcher, so only vblank work — palette cycles, the rain band — goes on). Players,
/// objects, spells, animations and the pause menu stop; the room scripts keep running.
#[derive(Resource, Default)]
pub struct WorldFreeze(pub bool);

/// Run condition: no room script is holding the game.
pub fn world_running(freeze: Res<WorldFreeze>) -> bool {
    !freeze.0
}

/// Sprites spawned by the scripts.
#[derive(Component)]
pub struct SilkThread;
#[derive(Component)]
pub struct BridgeBlock;
#[derive(Component)]
pub struct RainBand;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn patch_attrs(state: &mut RoomEventState, rooms: &mut Assets<RoomDef>, id: AssetId<RoomDef>, cells: &[[i32; 2]], attr: u8) {
    let Some(mut def) = rooms.get_mut(id) else { return };
    for &[x, y] in cells {
        if x < 0 || y < 0 || x >= def.blocks_w as i32 || y >= def.blocks_h as i32 {
            continue;
        }
        let i = y as usize * def.blocks_w as usize + x as usize;
        if !state.attr_backup.iter().any(|(c, _)| *c == i) {
            state.attr_backup.push((i, def.attrs[i]));
        }
        def.attrs[i] = attr;
    }
    state.patched = Some(id);
}

fn restore_attrs(state: &mut RoomEventState, rooms: &mut Assets<RoomDef>) {
    if let Some(id) = state.patched.take() {
        if let Some(mut def) = rooms.get_mut(id) {
            for &(i, a) in &state.attr_backup {
                def.attrs[i] = a;
            }
        }
    }
    state.attr_backup.clear();
}

fn spawn_mask(commands: &mut Commands, image: Handle<Image>, cycle: usize, parent: Option<Entity>) {
    let mut e = commands.spawn((Sprite::from_image(image), Anchor::TOP_LEFT, CycleMask { cycle }));
    match parent {
        Some(p) => {
            e.insert((Transform::from_xyz(0.0, 0.0, 0.1), ChildOf(p)));
        }
        None => {
            e.insert((Transform::from_translation(to_bevy(0, 0, z::FOREGROUND + 0.1)), ForegroundLayer { high: false }, RoomEntity));
        }
    }
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// Room-entry hook (cartridge 0x30EC6 plus the state reset of 0x47FA): runs once the room's layers
/// have been spawned.
#[allow(clippy::too_many_arguments)]
fn on_room_entry(
    events: Res<RoomEventsHandle>,
    defs: Res<Assets<RoomEventsDef>>,
    room: Option<Res<CurrentRoom>>,
    mut rooms: ResMut<Assets<RoomDef>>,
    cam: Res<CameraPos>,
    story: Res<StoryFlags>,
    fx: Res<Effects>,
    mut dialog: ResMut<Dialog>,
    mut state: ResMut<RoomEventState>,
    mut freeze: ResMut<WorldFreeze>,
    mut layers: Query<&mut BackgroundLayer>,
    mut fg: Query<(&ForegroundLayer, &mut Sprite), Without<CycleMask>>,
    mut players: Query<(&mut Player, &mut SpriteAnimator)>,
) {
    // A freshly spawned background layer means the room's layers were just (re)built.
    if !layers.iter_mut().any(|l| l.is_added()) {
        return;
    }
    let (Some(room), Some(ev)) = (room, defs.get(&events.0)) else { return };
    restore_attrs(&mut state, &mut rooms);
    *state = RoomEventState {
        room: Some(room.index),
        prev_cam_x: cam.x,
        entry_camera: IVec2::new(cam.x, cam.y),
        bg_variant: "default".into(),
        shake_step: ev.bg_swaps.iter().find(|s| s.room == room.index).and_then(|s| s.shake.as_ref()).map_or(3, |s| s.amplitude),
        rain_y: ev.graveyard.rain.y0,
        ..default()
    };
    freeze.0 = false;
    let r = room.index;
    let id = room.def.id();

    // 0xFF0B99 freeze bits.
    if let Some(f) = ev.entry_freeze.iter().find(|f| f.room == r) {
        state.freeze_bits = f.bits;
        for mut l in &mut layers {
            if f.bits & 1 != 0 {
                l.factor.x = 0.0;
            }
            if f.bits & 2 != 0 {
                l.factor.y = 0.0;
            }
        }
    }
    for m in ev.entry_messages.iter().filter(|m| m.room == r) {
        if !m.unless_flag.as_deref().is_some_and(|f| word_flag(&story, &fx, f)) {
            dialog.open(m.message, m.shape);
        }
    }
    // Player sprite priority (player +0x12 = 0x8000 in room 22, 0 elsewhere).
    let high = ev.player_priority_rooms.contains(&r);
    // Room 47 (0x30FEE): walk speed 1.0 for the scripted walk, poison cured.
    let er = &ev.ending_room;
    for (mut p, mut a) in &mut players {
        a.z = if high { z::FOREGROUND_HIGH + 2.0 } else { z::PLAYER };
        p.walk_speed = if r == er.room { er.walk_speed } else { None };
        if r == er.room && er.clear_poison {
            p.poisoned = false;
        }
    }
    // Room 19: the flame bridge only exists once built.
    let g = &ev.graveyard;
    if r == g.room {
        let b = &g.bridge;
        if word_flag(&story, &fx, &b.done_flag) {
            patch_attrs(&mut state, &mut rooms, id, &b.wall_cells, 0);
            state.bridge = BridgePhase::Done;
        } else {
            let cells: Vec<[i32; 2]> = (0..b.cells).map(|k| [b.col0 + k, b.row]).collect();
            patch_attrs(&mut state, &mut rooms, id, &cells, b.closed_attr);
            for (layer, mut sprite) in &mut fg {
                if !layer.high {
                    sprite.image = ev.image(&b.fg_closed);
                }
            }
        }
    }
    // Room 20: the wall stays open once Alex told you about it.
    for w in ev.wall_unlock.iter().filter(|w| w.room == r) {
        if word_flag(&story, &fx, &w.done_flag) {
            patch_attrs(&mut state, &mut rooms, id, &w.cells, w.attr);
        }
    }
}

/// Keeps the camera still in rooms with 0xFF0B99 bits 2/3 (boss arenas, the ending room).
fn camera_freeze(state: Res<RoomEventState>, mut cam: ResMut<CameraPos>) {
    if state.room.is_some() {
        if state.freeze_bits & 4 != 0 {
            cam.x = state.entry_camera.x;
        }
        if state.freeze_bits & 8 != 0 {
            cam.y = state.entry_camera.y;
        }
    }
}

/// Resources used by [`dispatch`].
#[derive(bevy::ecs::system::SystemParam)]
struct DispatchRes<'w> {
    clock: Res<'w, GameClock>,
    events: Res<'w, RoomEventsHandle>,
    defs: Res<'w, Assets<RoomEventsDef>>,
    room: Option<Res<'w, CurrentRoom>>,
    rooms: ResMut<'w, Assets<RoomDef>>,
    cam: ResMut<'w, CameraPos>,
    story: ResMut<'w, StoryFlags>,
    fx: ResMut<'w, Effects>,
    dialog: ResMut<'w, Dialog>,
    demo: ResMut<'w, DemoInput>,
    state: ResMut<'w, RoomEventState>,
    cycles: ResMut<'w, CycleState>,
    freeze: ResMut<'w, WorldFreeze>,
    magic: ResMut<'w, Magic>,
}

/// The per-room dispatcher (cartridge 0x31082). Runs only while the game is not paused.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn dispatch(
    mut commands: Commands,
    res: DispatchRes,
    mut players: Query<(&mut Player, &mut WorldPos, &mut Velocity, &Contacts)>,
    mut shots: Query<&mut WorldPos, (Or<(With<SpellProj>, With<WeaponShot>)>, Without<Player>)>,
    mut layers: Query<(&mut BackgroundLayer, &Children)>,
    mut tiles: Query<(&mut Sprite, Option<&Children>), With<BackgroundTile>>,
    masks: Query<(Entity, Has<ForegroundLayer>), With<CycleMask>>,
    mut fg: Query<(&ForegroundLayer, &mut Sprite), (Without<CycleMask>, Without<BackgroundTile>)>,
    silk_q: Query<Entity, With<SilkThread>>,
    blocks_q: Query<Entity, With<BridgeBlock>>,
) {
    let DispatchRes { clock, events, defs, room, mut rooms, mut cam, mut story, mut fx, mut dialog, mut demo, mut state, mut cycles, mut freeze, mut magic } =
        res;
    let (Some(room), Some(ev)) = (room, defs.get(&events.0)) else { return };
    let r = room.index;
    if state.room != Some(r) {
        return; // entry hook not run yet
    }
    let id = room.def.id();
    let block = IVec2::new(cam.x >> 4, cam.y >> 4);
    let cam_dx = cam.x - state.prev_cam_x;
    let block_changed = block.x != state.last_block;
    state.last_block = block.x;
    let odd = clock.tick & 1 == 1;

    // ---- Room 2: background swaps and shake (0x310FA) ----
    if let Some(swap) = ev.bg_swaps.iter().find(|s| s.room == r) {
        if block_changed {
            let showing_default = state.bg_variant == "default";
            let hit = swap.triggers.iter().find(|t| {
                block.y >= t.cam_block_y[0]
                    && block.y < t.cam_block_y[1]
                    && t.cam_block_x.contains(&block.x)
                    && if t.from == "default" { showing_default } else { !showing_default }
            });
            if let Some(v) = hit.and_then(|t| swap.variants.get(&t.variant).map(|v| (t.variant.clone(), v))) {
                let (name, v) = v;
                state.bg_variant = name;
                state.bg_offset = IVec2::new(-v.hscroll, v.vscroll);
                let image = ev.image(&v.image);
                let mut tile_entities = vec![];
                for (mut layer, children) in &mut layers {
                    layer.entry_camera = IVec2::new(cam.x, cam.y);
                    layer.factor = Vec2::new(if v.freeze[0] { 0.0 } else { 0.5 }, if v.freeze[1] { 0.0 } else { 0.5 });
                    for c in children.iter() {
                        if let Ok((mut sprite, _)) = tiles.get_mut(c) {
                            sprite.image = image.clone();
                            tile_entities.push(c);
                        }
                    }
                }
                // Replace the palette-cycle masks with the new set's.
                for (e, _) in &masks {
                    commands.entity(e).despawn();
                }
                for (i, c) in v.cycles.iter().enumerate() {
                    for (layer, path) in &c.masks {
                        if layer == "bg" {
                            for &t in &tile_entities {
                                spawn_mask(&mut commands, ev.image(path), i, Some(t));
                            }
                        } else {
                            spawn_mask(&mut commands, ev.image(path), i, None);
                        }
                    }
                }
                cycles.override_cycles = Some(v.cycles.clone());
                cycles.steps = vec![(0, 0); v.cycles.len()];
            }
        }
        // Earthquake: odd frames toggle both planes' vertical scroll by +-amplitude (0x31286).
        if let Some(sh) = &swap.shake {
            if odd && block.y >= sh.cam_block_y_min {
                let inside = block.x >= sh.cam_block_x[0] && block.x < sh.cam_block_x[1];
                if inside || state.shake_step < 0 {
                    state.shake += state.shake_step;
                    state.shake_step = -state.shake_step;
                }
            }
        }
    }

    // ---- Rooms 4 / 27: jingle once a story flag is set (0x312DA, 0x31964) ----
    for j in ev.flag_jingles.iter().filter(|j| j.room == r) {
        if !word_flag(&story, &fx, &j.done_flag) && word_flag(&story, &fx, &j.flag) {
            fx.sounds.push(j.sound);
            fx.set_global(0xFF0A56, j.music_timer);
            set_word_flag(&mut story, &j.done_flag);
        }
    }

    // ---- Room 20: the wall opens (0x31874) ----
    for w in ev.wall_unlock.iter().filter(|w| w.room == r) {
        if !word_flag(&story, &fx, &w.done_flag) && word_flag(&story, &fx, &w.flag) {
            patch_attrs(&mut state, &mut rooms, id, &w.cells, w.attr);
            set_word_flag(&mut story, &w.done_flag);
        }
    }

    // ---- Room 12: looping forest (0x3130A) and Lilliput Orchid tiles (0xC45A) ----
    let fl = &ev.forest_loop;
    if r == fl.room && block_changed && cam_dx < 0 && block.x == fl.cam_block_x {
        let shift = fl.shift_blocks * 16;
        cam.x += shift;
        for (p, mut pos, _, _) in &mut players {
            if p.state < PlayerState::Dying {
                pos.x += px(shift);
            }
        }
        for mut pos in &mut shots {
            pos.x += px(shift);
        }
        // The Flame Wall / Blizzard anchors (0xFF0B6A / 0xFF0B7A) move with the spell.
        magic.shift_x(shift);
        for (mut layer, _) in &mut layers {
            layer.entry_camera.x += shift;
        }
    }
    let gn = &ev.gnome;
    if r == gn.room {
        if state.gnome_pending {
            // The orchid's message has been read: new animation set and sound (0x31408).
            state.gnome_pending = false;
            fx.sounds.push(gn.sound);
        }
        if word_flag(&story, &fx, &gn.item_flag) {
            if let Some(def) = rooms.get(id) {
                for (mut p, pos, mut vel, _) in &mut players {
                    if p.state >= PlayerState::Dying {
                        continue;
                    }
                    let a = def.attr_at(pos.px_x(), pos.px_y() - 1);
                    if a == state.gnome_last_attr {
                        continue;
                    }
                    state.gnome_last_attr = a;
                    let grow = a == gn.grow_attr && p.small;
                    let shrink = a == gn.shrink_attr && !p.small;
                    if grow || shrink {
                        p.small = shrink;
                        p.state = PlayerState::Stand;
                        p.action = Action::None;
                        *vel = Velocity::default();
                    }
                    if shrink {
                        dialog.open(gn.message, gn.message_shape);
                        state.gnome_pending = true;
                    }
                }
            }
        }
    }

    // ---- Room 13: Silk of the Worm (0x31432) ----
    let sr = &ev.silk_rope;
    if r == sr.room {
        match state.silk {
            SilkPhase::Idle => {
                if block_changed && block.x == sr.cam_block_x && block.y >= sr.cam_block_y[0] && block.y < sr.cam_block_y[1] {
                    if word_flag(&story, &fx, &sr.item_flag) {
                        dialog.open(sr.message_ok, sr.message_ok_shape);
                        state.silk = SilkPhase::Descending;
                        state.silk_y = sr.start[1];
                        commands.spawn((
                            SilkThread,
                            Sprite::from_image(ev.image(&sr.sprite.image)),
                            Anchor::TOP_LEFT,
                            Transform::from_translation(to_bevy(sr.start[0] - sr.sprite.origin[0], sr.start[1] - sr.sprite.origin[1], z::ACTORS - 1.0)),
                            RoomEntity,
                        ));
                    } else {
                        dialog.open(sr.message_fail, sr.message_fail_shape);
                        state.silk = SilkPhase::Refused;
                    }
                }
            }
            SilkPhase::Descending => {
                if state.silk_y >= sr.end_y {
                    patch_attrs(&mut state, &mut rooms, id, &sr.rope_cells, sr.rope_attr);
                    state.silk = SilkPhase::Down;
                } else {
                    state.silk_y += sr.step;
                }
            }
            SilkPhase::Down | SilkPhase::Refused => {}
        }
        for e in &silk_q {
            commands.entity(e).insert(Transform::from_translation(to_bevy(
                sr.start[0] - sr.sprite.origin[0],
                state.silk_y - sr.sprite.origin[1],
                z::ACTORS - 1.0,
            )));
        }
    }

    // ---- Room 19: flame bridge, rain switch, Abel's grave (0x31562) ----
    let g = &ev.graveyard;
    if r == g.room {
        let b = &g.bridge;
        let mut run_common = true;
        match state.bridge {
            BridgePhase::Idle => {
                if !word_flag(&story, &fx, &b.done_flag) && block_changed && block.x == b.cam_block_x && cam_dx >= 0 {
                    if word_flag(&story, &fx, &b.item_flag) {
                        dialog.open(b.message_ok, b.message_ok_shape);
                        state.bridge = BridgePhase::Flute;
                    } else {
                        dialog.open(b.message_fail, b.message_fail_shape);
                        state.bridge = BridgePhase::Done;
                    }
                }
            }
            BridgePhase::Flute => {
                fx.sounds.push(b.sound);
                fx.set_global(0xFF0A56, b.wait_frames as i32);
                state.bridge = BridgePhase::Waiting(b.wait_frames);
                // From the next frame on the cartridge loops inside the handler (0x315D4).
                freeze.0 = true;
            }
            BridgePhase::Waiting(n) => {
                // The cartridge loops inside the handler here: the game stands still.
                run_common = false;
                state.bridge = if n > 1 { BridgePhase::Waiting(n - 1) } else { BridgePhase::Building(0) };
            }
            BridgePhase::Building(k) => {
                run_common = false;
                if clock.tick % b.block_every == b.block_every - 1 {
                    if k < b.blocks_drawn {
                        commands.spawn((
                            BridgeBlock,
                            Sprite::from_image(ev.image(&b.block_image)),
                            Anchor::TOP_LEFT,
                            Transform::from_translation(to_bevy((b.col0 + k) * 16, b.row * 16, z::FOREGROUND + 0.2)),
                            RoomEntity,
                        ));
                        state.bridge = BridgePhase::Building(k + 1);
                    } else {
                        // Map cells become bridge blocks, the invisible wall loses its attribute.
                        let cells: Vec<[i32; 2]> = (0..b.cells).map(|k| [b.col0 + k, b.row]).collect();
                        patch_attrs(&mut state, &mut rooms, id, &cells, b.open_attr);
                        patch_attrs(&mut state, &mut rooms, id, &b.wall_cells, 0);
                        if let Some(def) = rooms.get(id) {
                            for (layer, mut sprite) in &mut fg {
                                if !layer.high {
                                    sprite.image = def.fg_image.clone();
                                }
                            }
                        }
                        for e in &blocks_q {
                            commands.entity(e).despawn();
                        }
                        set_word_flag(&mut story, &b.done_flag);
                        state.bridge = BridgePhase::Done;
                        freeze.0 = false;
                    }
                }
            }
            BridgePhase::Done => {}
        }
        if run_common {
            // Rain band on/off (0xFF0BAE), keyed on the direction the camera moves.
            let rn = &g.rain;
            if block.x != state.last_block_rain {
                state.last_block_rain = block.x;
                if cam_dx >= 0 {
                    if block.x == rn.on_right {
                        state.rain_on = true;
                    } else if block.x == rn.off_right {
                        state.rain_on = false;
                    }
                } else if block.x == rn.on_left {
                    state.rain_on = true;
                } else if block.x == rn.off_left {
                    state.rain_on = false;
                }
            }
            // Standing on the grave (attribute 0x70) reads it or places Gina's charm.
            let gr = &g.grave;
            if let Some(def) = rooms.get(id) {
                for (p, pos, _, c) in &players {
                    if p.state >= PlayerState::Dying || !c.ground {
                        continue;
                    }
                    let a = def.attr_at(pos.px_x(), pos.px_y() - 1);
                    if a == state.grave_last_attr {
                        continue;
                    }
                    state.grave_last_attr = a;
                    if a == gr.attr && !word_flag(&story, &fx, &gr.done_flag) {
                        if word_flag(&story, &fx, &gr.item_flag) {
                            dialog.open(gr.message_charm, gr.message_charm_shape);
                        } else {
                            dialog.open(gr.message_read, gr.message_read_shape);
                        }
                    }
                }
            }
        }
    }

    // ---- Room 47: the ending's second step (0x31994) ----
    let er = &ev.ending_room;
    if r == er.room {
        let t = state.ending_timer.unwrap_or(0);
        if t != u32::MAX {
            let t = t + 1;
            if t >= er.delay {
                demo.start(er.demo.clone());
                dialog.open_timed(er.message, er.shape, er.timed[0], er.timed[1]);
                state.ending_timer = Some(u32::MAX);
            } else {
                state.ending_timer = Some(t);
            }
        }
    }

    state.prev_cam_x = cam.x;
}

/// Room 19's rain band, stepped every vblank by the cartridge (0x3183A), drawn in screen space.
fn rain(
    mut commands: Commands,
    events: Res<RoomEventsHandle>,
    defs: Res<Assets<RoomEventsDef>>,
    mut state: ResMut<RoomEventState>,
    roots: Query<Entity, With<ScreenRoot>>,
    mut band: Query<(Entity, &mut Transform), With<RainBand>>,
) {
    let Some(ev) = defs.get(&events.0) else { return };
    let rn = &ev.graveyard.rain;
    let active = state.room == Some(ev.graveyard.room) && state.rain_on;
    if !active {
        state.rain_y = rn.y0;
        for (e, _) in &band {
            commands.entity(e).despawn();
        }
        return;
    }
    let y = state.rain_y;
    let at = Vec3::new((rn.x - rn.origin[0]) as f32, -((y - rn.origin[1]) as f32), z::FOREGROUND + 5.0);
    if let Ok((_, mut tf)) = band.single_mut() {
        tf.translation = at;
    } else if let Ok(root) = roots.single() {
        commands.spawn((RainBand, Sprite::from_image(ev.image(&rn.image)), Anchor::TOP_LEFT, Transform::from_translation(at), RoomEntity, ChildOf(root)));
    }
    state.rain_y += rn.step;
    if state.rain_y >= rn.y_end {
        state.rain_y = rn.y0;
    }
}

/// Applies the room-2 background variant offset and the vertical shake to both planes.
fn apply_plane_offsets(
    state: Res<RoomEventState>,
    mut layers: Query<&mut BackgroundLayer>,
    mut fg: Query<(&mut Transform, &ForegroundLayer)>,
) {
    if state.room.is_none() {
        return;
    }
    for mut l in &mut layers {
        let want = state.bg_offset + IVec2::new(0, state.shake);
        if l.offset != want {
            l.offset = want;
        }
    }
    // Vertical scroll +s moves the plane up by s pixels.
    for (mut tf, _) in &mut fg {
        let y = state.shake as f32;
        if tf.translation.y != y {
            tf.translation.y = y;
        }
    }
}

fn load_events(mut commands: Commands, server: Res<AssetServer>, mut story: ResMut<StoryFlags>) {
    commands.insert_resource(RoomEventsHandle(server.load(EVENTS_PATH)));
    // Testing aid: `--flags 0xFF0ACE,0xFF0AD8` pre-sets cartridge word flags (key items, story).
    let args: Vec<String> = std::env::args().collect();
    if let Some(list) = args.iter().position(|a| a == "--flags").and_then(|i| args.get(i + 1)) {
        for f in list.split(',') {
            let a = parse_addr(f) & !1;
            story.0.insert(a, 1);
            story.0.insert(a + 1, 1);
        }
    }
}

fn not_paused(
    dialog: Res<Dialog>,
    windows: Res<super::dialog::Windows>,
    transition: Option<Res<super::transition::Transition>>,
) -> bool {
    !dialog.is_open() && !windows.any_open() && transition.is_none()
}

pub fn plugin(app: &mut App) {
    app.init_asset::<RoomEventsDef>()
        .register_asset_loader(JsonLoader::<RoomEventsDef>::new(&["events.json"]))
        .init_resource::<RoomEventState>()
        .init_resource::<WorldFreeze>()
        .add_systems(Startup, load_events)
        .add_systems(
            FixedUpdate,
            (
                on_room_entry,
                camera_freeze.after(camera::follow_player),
                dispatch.run_if(not_paused),
                rain,
            )
                .chain()
                .in_set(GameSystems::Camera)
                .after(camera::follow_player)
                .run_if(in_state(AppState::InGame)),
        )
        .add_systems(PostUpdate, apply_plane_offsets.before(super::room::scroll_background));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_data_parses() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/", "data/room_events.events.json");
        let d: RoomEventsDef = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let swap = &d.bg_swaps[0];
        assert_eq!(swap.room, 2);
        assert_eq!(swap.variants["upper"].hscroll, 0x1A0);
        assert_eq!(swap.variants["lower"].vscroll, 0x40);
        assert_eq!(d.graveyard.bridge.col0, 186);
        assert_eq!(d.silk_rope.rope_cells.len(), 8);
    }

    #[test]
    fn word_flags_accept_either_byte() {
        let mut story = StoryFlags::default();
        let fx = Effects::default();
        assert!(!word_flag(&story, &fx, "0xFF0ADE"));
        story.0.insert(0xFF0ADF, 1);
        assert!(word_flag(&story, &fx, "0xFF0ADE"));
    }
}
