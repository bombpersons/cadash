//! Run-time sprite palette effects (docs/re/palette_effects.md).
//!
//! Object sheets are exported twice: a coloured atlas (the colours each frame has in its graphics
//! context) and a palette-indexed copy plus the CRAM words of every context. While the sprite palette
//! is in its default state the coloured atlas is used as is; otherwise each sheet's atlas is
//! recoloured from the indexed copy (cached per state, so toggling a flash is a handle swap).
//!
//! Cartridge behaviour modelled here (only CRAM line 2, which only object sprites use):
//! * boss hit flash: line 2 colours 1..15 `| 0x888` (0x1431A) until the boss's flash counter
//!   reaches 6, then line 2 is restored from the fade target (0x1433E);
//! * graphics-bank switches by boss scripts load a new line 2 for every sprite (0x2E14 into the
//!   current and target buffers, which also ends a running flash);
//! * room 24: when the boss script sets 0xFF0AEC, line 2 := 0x4BCA (0x3189A);
//! * room 27: on a change of the camera block row while the camera block x ≤ 12: row 13 → castle
//!   palette 0x4ACA, row 14 → back to 0x4BEA (0x318CA).
//!
//! Every room load reloads the room's palette (0x4900), so the state resets on room changes.
use super::{
    AppState,
    camera::CameraPos,
    data::{RoomDef, RoomObjects, SpriteSheet},
    objects::{objects_running, ops::Effects},
    player_look::{PaletteHandle, PlayerPaletteDef},
    room::CurrentRoom,
    room_events::RoomEventsHandle,
};
use crate::GameSystems;
use bevy::{
    asset::RenderAssetUsages,
    image::ImageSampler,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use serde::Deserialize;
use std::collections::HashMap;

/// First CRAM index of palette line 2.
const LINE2: usize = 32;
/// Boss hit flash: OR mask for line 2 colours 1..15 (0x1431A).
pub const FLASH_OR: u16 = 0x888;

/// Changes requested by object scripts (through `Effects::palette_ops`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteOp {
    /// A boss's hit-flash counter is 1: brighten line 2.
    FlashOn,
    /// The counter reached 6: restore line 2.
    FlashOff,
    /// An object switched to another graphics context (bank switch with its own line 2).
    Context(String),
}

/// Records a graphics-context switch made while an object ran.
pub fn note_context_change(fx: &mut Effects, before: &str, after: &str) {
    if before != after {
        fx.palette_ops.push(PaletteOp::Context(after.to_owned()));
    }
}

/// One entry of `palette_line2_swaps` in `data/room_events.events.json`.
#[derive(Deserialize, Debug, Clone)]
pub struct Line2SwapDef {
    pub room: usize,
    /// Word flag that triggers the swap (and is cleared by it).
    #[serde(default)]
    pub flag: Option<String>,
    /// Camera-row trigger: the swap applies when the camera block row changes to this value...
    #[serde(default)]
    pub cam_block_y: Option<i32>,
    /// ...while the camera block column is at most this.
    #[serde(default)]
    pub cam_block_x_max: Option<i32>,
    /// CRAM words of the new line 2.
    #[serde(default)]
    pub words: Vec<u16>,
}

/// The sprite palette state (cartridge CRAM line 2 as far as it differs from the exported sheets).
#[derive(Resource, Default, Debug, Clone, PartialEq, Eq)]
pub struct SpritePalette {
    /// Line 2 loaded at run time; `None` = every frame keeps the line 2 it was exported with.
    pub line2: Option<[u16; 16]>,
    /// Boss hit flash active (line 2 colours 1..15 | 0x888).
    pub flash: bool,
}

impl SpritePalette {
    /// Loads a new line 2 into both palette buffers (ends a running flash).
    pub fn load_line2(&mut self, words: &[u16]) {
        let mut l = [0u16; 16];
        for (d, s) in l.iter_mut().zip(words) {
            *d = *s;
        }
        self.line2 = Some(l);
        self.flash = false;
    }
    fn key(&self) -> PalKey {
        (self.line2, self.flash)
    }
}

type PalKey = (Option<[u16; 16]>, bool);

/// Room-script trackers (cartridge 0xFF0B94 / 0xFF0BB0, cleared on room change).
#[derive(Resource, Default)]
struct SwapState {
    room: Option<usize>,
    last_block_y: Option<i32>,
    castle: bool,
}

/// Recoloured atlases per (sheet, palette state) and what each sheet shows now.
#[derive(Resource, Default)]
struct Recoloured {
    applied: HashMap<AssetId<SpriteSheet>, PalKey>,
    cache: HashMap<(AssetId<SpriteSheet>, PalKey), Handle<Image>>,
}

/// The current room's object sheet.
fn room_sheet(room: &CurrentRoom, rooms: &Assets<RoomDef>, objects: &Assets<RoomObjects>) -> Option<AssetId<SpriteSheet>> {
    let def = rooms.get(&room.def)?;
    Some(objects.get(&def.objects_handle)?.sheet_handle.id())
}

/// Resets on room change, applies script requests and the room-24/27 swaps.
#[allow(clippy::too_many_arguments)]
fn update_palette(
    room: Option<Res<CurrentRoom>>,
    rooms: Res<Assets<RoomDef>>,
    objects: Res<Assets<RoomObjects>>,
    sheets: Res<Assets<SpriteSheet>>,
    events: Option<Res<RoomEventsHandle>>,
    defs: Res<Assets<super::room_events::RoomEventsDef>>,
    cam: Res<CameraPos>,
    mut fx: ResMut<Effects>,
    mut pal: ResMut<SpritePalette>,
    mut swap: ResMut<SwapState>,
    mut rc: ResMut<Recoloured>,
) {
    let Some(room) = room else { return };
    if swap.room != Some(room.index) {
        *swap = SwapState { room: Some(room.index), ..default() };
        *pal = SpritePalette::default();
        rc.cache.clear();
        fx.palette_ops.clear();
    }
    // Object scripts (in request order).
    let objs_sheet = room_sheet(&room, &rooms, &objects).and_then(|id| sheets.get(id));
    for op in std::mem::take(&mut fx.palette_ops) {
        match op {
            PaletteOp::FlashOn => pal.flash = true,
            PaletteOp::FlashOff => pal.flash = false,
            PaletteOp::Context(name) => {
                let words = objs_sheet.and_then(|s| {
                    let ix = s.indexed.as_ref()?;
                    let k = ix.contexts.iter().position(|c| *c == name)?;
                    ix.palettes.get(k).map(|p| p[LINE2..LINE2 + 16].to_vec())
                });
                if let Some(w) = words {
                    pal.load_line2(&w);
                }
            }
        }
    }
    // Room scripts (dispatcher 0x31082).
    let Some(ev) = events.and_then(|h| defs.get(&h.0)) else { return };
    let block = IVec2::new(cam.x >> 4, cam.y >> 4);
    let row_changed = swap.last_block_y != Some(block.y);
    swap.last_block_y = Some(block.y);
    for (k, s) in ev.palette_line2_swaps.iter().filter(|s| s.room == room.index).enumerate() {
        if let Some(flag) = &s.flag {
            let addr = u32::from_str_radix(flag.trim_start_matches("0x"), 16).unwrap_or(0);
            if fx.global(addr) != 0 {
                pal.load_line2(&s.words);
                fx.globals.insert(addr, 0);
            }
        }
        if let (Some(y), Some(xmax)) = (s.cam_block_y, s.cam_block_x_max) {
            // The first camera-row swap (row 13: castle palette) applies once, the next one (row 14)
            // switches back (toggle 0xFF0BB0).
            let to_castle = k == 0;
            if row_changed && block.x <= xmax && block.y == y && swap.castle != to_castle {
                pal.load_line2(&s.words);
                swap.castle = to_castle;
            }
        }
    }
}

/// Points the room's object sheet `image` at the atlas matching the current palette state.
#[allow(clippy::too_many_arguments)]
fn recolour_sheets(
    room: Option<Res<CurrentRoom>>,
    rooms: Res<Assets<RoomDef>>,
    objects: Res<Assets<RoomObjects>>,
    pal: Res<SpritePalette>,
    ladder: Option<Res<PaletteHandle>>,
    ladders: Res<Assets<PlayerPaletteDef>>,
    mut sheets: ResMut<Assets<SpriteSheet>>,
    mut images: ResMut<Assets<Image>>,
    mut rc: ResMut<Recoloured>,
) {
    let key = pal.key();
    let Some(id) = room.and_then(|r| room_sheet(&r, &rooms, &objects)) else { return };
    let pending: Vec<AssetId<SpriteSheet>> =
        sheets.get(id).filter(|s| s.indexed.is_some() && rc.applied.get(&id) != Some(&key)).map(|_| id).into_iter().collect();
    if pending.is_empty() {
        return;
    }
    let Some(ladder) = ladder.and_then(|h| ladders.get(&h.0)) else { return };
    for id in pending {
        let Some(sheet) = sheets.get(id) else { continue };
        let handle = if key == (None, false) {
            sheet.base_image.clone()
        } else if let Some(h) = rc.cache.get(&(id, key)) {
            h.clone()
        } else {
            let Some(ix) = sheet.indexed.as_ref() else { continue };
            let Some(img) = images.get(&ix.image).and_then(|src| recolour(src, &ix.palettes, &pal, ladder)) else {
                continue; // indexed atlas not loaded yet
            };
            let h = images.add(img);
            rc.cache.insert((id, key), h.clone());
            h
        };
        if let Some(mut s) = sheets.get_mut(id) {
            s.image = handle;
        }
        rc.applied.insert(id, key);
    }
}

/// Colours an indexed atlas (R = CRAM index, G = graphics context) with the context palettes,
/// line 2 replaced / flashed as the palette state says.
fn recolour(src: &Image, palettes: &[Vec<u16>], pal: &SpritePalette, ladder: &PlayerPaletteDef) -> Option<Image> {
    let data = src.data.as_ref()?;
    let (w, h) = (src.width(), src.height());
    let n = (w * h) as usize;
    let bpp = data.len() / n.max(1);
    if bpp < 2 {
        return None;
    }
    let luts: Vec<[[u8; 4]; 64]> = palettes
        .iter()
        .map(|words| {
            let mut lut = [[0u8; 4]; 64];
            for (i, e) in lut.iter_mut().enumerate() {
                let mut word = words.get(i).copied().unwrap_or(0);
                if (LINE2..LINE2 + 16).contains(&i) {
                    if let Some(l2) = pal.line2 {
                        word = l2[i - LINE2];
                    }
                    if pal.flash && i != LINE2 {
                        word |= FLASH_OR;
                    }
                }
                let [r, g, b] = ladder.rgb(word);
                *e = [r, g, b, 255];
            }
            lut
        })
        .collect();
    let mut out = vec![0u8; n * 4];
    for (i, px) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let idx = data[i * bpp] as usize;
        if idx == 0 {
            continue;
        }
        let ctx = (data[i * bpp + 1] as usize).min(luts.len().saturating_sub(1));
        if let Some(lut) = luts.get(ctx) {
            *px = lut[idx & 63];
        }
    }
    let mut img = Image::new(
        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        TextureDimension::D2,
        out,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.sampler = ImageSampler::nearest();
    Some(img)
}

pub fn plugin(app: &mut App) {
    app.init_resource::<SpritePalette>()
        .init_resource::<SwapState>()
        .init_resource::<Recoloured>()
        .add_systems(
            FixedUpdate,
            update_palette
                .in_set(GameSystems::Camera)
                .after(super::camera::follow_player)
                .run_if(in_state(AppState::InGame))
                .run_if(objects_running),
        )
        .add_systems(PostUpdate, recolour_sheets.before(super::anim::sync_sprites).before(super::objects::render_pieces));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swaps_parse_with_words() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/", "data/room_events.events.json");
        let d: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let swaps: Vec<Line2SwapDef> = serde_json::from_value(d["palette_line2_swaps"].clone()).unwrap();
        assert_eq!(swaps.len(), 3);
        assert!(swaps.iter().all(|s| s.words.len() == 16));
        assert_eq!(swaps[0].room, 24);
        assert_eq!(swaps[1].cam_block_y, Some(13));
    }
}
