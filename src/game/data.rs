//! Data assets extracted from the original cartridge (see `extract/` in the repo root).
//!
//! Everything here is plain serde data loaded through Bevy's asset server, so content can
//! be edited or replaced without touching game code.
use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    prelude::*,
};
use serde::{Deserialize, de::DeserializeOwned};
use std::{collections::HashMap, marker::PhantomData};

/// Generic JSON asset loader keyed on a double extension such as `room.json`.
#[derive(TypePath)]
pub struct JsonLoader<T: TypePath> {
    extensions: &'static [&'static str],
    _marker: PhantomData<fn() -> T>,
}

impl<T: TypePath> JsonLoader<T> {
    pub fn new(extensions: &'static [&'static str]) -> Self {
        Self {
            extensions,
            _marker: PhantomData,
        }
    }
}

/// Assets that need to resolve dependent handles (e.g. atlas images) after parsing.
pub trait JsonAsset: Asset + DeserializeOwned {
    fn resolve(&mut self, _ctx: &mut LoadContext) {}
}

impl<T: JsonAsset + TypePath> AssetLoader for JsonLoader<T> {
    type Asset = T;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        load_context: &mut LoadContext<'_>,
    ) -> Result<T, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let mut asset: T = serde_json::from_slice(&bytes).map_err(std::io::Error::other)?;
        asset.resolve(load_context);
        Ok(asset)
    }

    fn extensions(&self) -> &[&str] {
        self.extensions
    }
}

// ---------------------------------------------------------------------------
// Sprite sheets
// ---------------------------------------------------------------------------

/// One composited animation frame inside an atlas image.
#[derive(Deserialize, Clone, Debug)]
pub struct SheetFrame {
    /// x, y, w, h inside the atlas.
    pub rect: [u32; 4],
    /// Position of the object's anchor point (its world position) inside the frame image.
    pub origin: [i32; 2],
    /// Hit boxes as stored in the cartridge: body [w, h, dx, dy], attack [w, h, dx, dy].
    /// A width/height of 30000 means "no box".
    #[serde(default)]
    pub boxes: [i16; 8],
    /// dx, dy of the ROM mapping's first piece (mapping+18/+20), read by some boss scripts.
    #[serde(default)]
    pub piece0: Option<[i32; 2]>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct AnimFrame {
    pub frame: usize,
    /// Duration in 60 Hz ticks. 0xFFFF = hold forever, 0x8000 = actor-defined duration.
    pub dur: u16,
}

#[derive(Deserialize, Clone, Debug)]
pub struct AnimDef {
    /// Frame index to loop back to after the last frame.
    #[serde(rename = "loop")]
    pub loop_to: usize,
    pub frames: Vec<AnimFrame>,
}

#[derive(Asset, TypePath, Deserialize, Debug)]
pub struct SpriteSheet {
    pub atlas: String,
    #[serde(skip)]
    pub image: Handle<Image>,
    pub frames: Vec<SheetFrame>,
    pub anims: HashMap<String, AnimDef>,
    /// Individual sprite pieces ("attr:size:gfx" → frame) for objects that move their pieces.
    #[serde(default)]
    pub pieces: HashMap<String, usize>,
    /// Piece lists of frames that such objects copy into their variables.
    #[serde(default)]
    pub frame_pieces: HashMap<String, FramePieces>,
    /// Palette-indexed copy of the atlas: lets run-time palette effects recolour the sheet
    /// (`sprite_palette`). `image` then points at the recoloured atlas, `base_image` keeps the original.
    #[serde(default)]
    pub indexed: Option<IndexedAtlas>,
    #[serde(skip)]
    pub base_image: Handle<Image>,
}

/// Indexed atlas (R = CRAM index 0..63, 0 = transparent; G = graphics context) and the 64 CRAM
/// words each context was rendered with.
#[derive(Deserialize, Clone, Debug)]
pub struct IndexedAtlas {
    pub atlas: String,
    pub contexts: Vec<String>,
    pub palettes: Vec<Vec<u16>>,
    #[serde(skip)]
    pub image: Handle<Image>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct FramePieces {
    /// Piece count word of the mapping (including disabled pieces).
    pub count: u16,
    /// dx, dy, size, attr per piece.
    pub pieces: Vec<[i32; 4]>,
}

impl JsonAsset for SpriteSheet {
    fn resolve(&mut self, ctx: &mut LoadContext) {
        let dir = ctx.path().path().parent().map(|p| p.to_path_buf()).unwrap_or_default();
        self.image = ctx.load(dir.join(&self.atlas));
        self.base_image = self.image.clone();
        if let Some(ix) = self.indexed.as_mut() {
            // Kept on the CPU (recoloured by `sprite_palette`), raw values (no sRGB conversion).
            ix.image = ctx
                .load_builder()
                .with_settings(|s: &mut bevy::image::ImageLoaderSettings| {
                    s.asset_usage = bevy::asset::RenderAssetUsages::MAIN_WORLD;
                    s.is_srgb = false;
                })
                .load(dir.join(&ix.atlas));
        }
    }
}

pub const HOLD: u16 = 0xFFFF;
pub const ACTOR_DURATION: u16 = 0x8000;
pub const NO_BOX: i16 = 30000;

// ---------------------------------------------------------------------------
// Rooms
// ---------------------------------------------------------------------------

/// Where an exit leads.
#[derive(Deserialize, Debug, Clone)]
pub struct ExitDest {
    pub room: usize,
    pub x: i32,
    pub y: i32,
    pub facing: String,
    /// Camera top-left in world pixels on arrival.
    pub camera: [i32; 2],
    #[serde(default)]
    pub music: u32,
    #[serde(default)]
    pub sound: Option<u32>,
}

/// In-room stair teleport (attribute 0x3N).
#[derive(Deserialize, Debug, Clone, Copy)]
pub struct StairDef {
    pub dy: i32,
    pub freeze: u32,
    pub iframes: u32,
}

/// Exits keyed by the low nibble of the triggering block attribute.
#[derive(Deserialize, Debug, Clone, Default)]
pub struct RoomExits {
    #[serde(default)]
    pub door: HashMap<String, ExitDest>,
    #[serde(default)]
    pub edge: HashMap<String, ExitDest>,
    #[serde(default)]
    pub warp: HashMap<String, ExitDest>,
    #[serde(default)]
    pub stairs: HashMap<String, StairDef>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct CycleStep {
    pub frames: u32,
    pub rgb: [u8; 3],
}

/// A palette entry that cycles through colours; drawn as tinted white masks.
#[derive(Deserialize, Debug, Clone)]
pub struct PaletteCycle {
    /// Layer name ("fg"/"bg") → mask image path.
    pub masks: HashMap<String, String>,
    pub steps: Vec<CycleStep>,
}

/// Enemy/NPC spawn marker (see docs/re/world.md §2).
#[derive(Deserialize, Debug, Clone)]
pub struct SpawnMarker {
    pub block: [i32; 2],
    pub flags: u32,
    pub kind: String,
    pub spawn_id: u32,
    pub spawn_world_px: [i32; 2],
    pub def_ptr: String,
}

#[derive(Asset, TypePath, Deserialize, Debug)]
pub struct RoomDef {
    pub index: usize,
    pub blocks_w: u32,
    pub blocks_h: u32,
    /// Collision/trigger attribute for every 16x16 block, row-major.
    pub attrs: Vec<u8>,
    pub backdrop: [u8; 3],
    pub fg: String,
    pub fg_hi: String,
    pub bg: String,
    /// Background parallax factor relative to the camera.
    pub bg_scroll: [f32; 2],
    #[serde(default)]
    pub palette_cycles: Vec<PaletteCycle>,
    #[serde(default)]
    pub music: u32,
    #[serde(default)]
    pub exits: RoomExits,
    #[serde(default)]
    pub spawns: Vec<SpawnMarker>,
    /// Object scripts, records and markers for this room.
    pub objects: String,
    #[serde(skip)]
    pub objects_handle: Handle<RoomObjects>,
    #[serde(skip)]
    pub fg_image: Handle<Image>,
    #[serde(skip)]
    pub fg_hi_image: Handle<Image>,
    #[serde(skip)]
    pub bg_image: Handle<Image>,
    /// (layer, mask image) per palette cycle, parallel to `palette_cycles`.
    #[serde(skip)]
    pub cycle_images: Vec<Vec<(String, Handle<Image>)>>,
}

impl JsonAsset for RoomDef {
    fn resolve(&mut self, ctx: &mut LoadContext) {
        self.fg_image = ctx.load(self.fg.clone());
        self.fg_hi_image = ctx.load(self.fg_hi.clone());
        self.bg_image = ctx.load(self.bg.clone());
        self.objects_handle = ctx.load(self.objects.clone());
        self.cycle_images = self
            .palette_cycles
            .iter()
            .map(|c| c.masks.iter().map(|(layer, path)| (layer.clone(), ctx.load(path.clone()))).collect())
            .collect();
    }
}

impl RoomDef {
    pub fn width_px(&self) -> i32 {
        self.blocks_w as i32 * 16
    }
    pub fn height_px(&self) -> i32 {
        self.blocks_h as i32 * 16
    }
    /// Attribute byte of the block containing world pixel (x, y). Outside the room counts as solid.
    pub fn attr_at(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.width_px() || y >= self.height_px() {
            return attr::SOLID;
        }
        let cell = (y / 16) as usize * self.blocks_w as usize + (x / 16) as usize;
        self.attrs.get(cell).copied().unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Objects (docs/re/objects.md)
// ---------------------------------------------------------------------------

/// One state of an object script: animation, per-tick behaviour opcode, entry opcode and
/// condition → next-state pairs evaluated in order.
#[derive(Deserialize, Debug, Clone)]
pub struct ObjState {
    pub anim: Option<u32>,
    pub behaviour: u16,
    pub init: u16,
    /// Graphics context (sprite bank switch) the state draws with.
    pub gfx: String,
    pub conds: Vec<[u16; 2]>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ObjType {
    pub name: String,
    pub states: HashMap<String, ObjState>,
}

/// Spawn record: what an object is and its stats.
#[derive(Deserialize, Debug, Clone, Copy, Default)]
pub struct SpawnRecord {
    #[serde(rename = "type")]
    pub ty: u16,
    pub cost: u16,
    pub hp: u16,
    pub exp: u16,
    pub gold: u16,
    pub death_type: u16,
    pub defence: u16,
    pub flags: u16,
    pub touch_dmg: u16,
    pub attack_dmg: u16,
    pub tile_ofs: u16,
    pub param118: u16,
}

#[derive(Deserialize, Debug, Clone, Copy)]
pub struct ObjMarker {
    pub cell: [i32; 2],
    pub persistent: bool,
    pub high_pool: bool,
    pub index: usize,
    pub guard_flag: Option<u8>,
}

/// Per-room object data (`objects/room_NN.objects.json`).
#[derive(Asset, TypePath, Deserialize, Debug)]
pub struct RoomObjects {
    pub sheet: String,
    #[serde(skip)]
    pub sheet_handle: Handle<SpriteSheet>,
    /// Collision box (w, h) per animation key.
    pub anim_collision: HashMap<String, [i32; 2]>,
    pub types: HashMap<String, ObjType>,
    pub records: Vec<SpawnRecord>,
    pub persistent_records: Vec<SpawnRecord>,
    pub markers: Vec<ObjMarker>,
}

impl JsonAsset for RoomObjects {
    fn resolve(&mut self, ctx: &mut LoadContext) {
        let dir = ctx.path().path().parent().map(|p| p.to_path_buf()).unwrap_or_default();
        self.sheet_handle = ctx.load(dir.join(&self.sheet));
    }
}

/// Parameter preset of a simple state-entry opcode (values in pixels per tick).
#[derive(Deserialize, Debug, Clone, Copy, Default)]
pub struct InitPreset {
    pub vx: Option<f64>,
    pub vy: Option<f64>,
    pub ax: Option<f64>,
    pub ay: Option<f64>,
    pub face_left: Option<u8>,
    pub despawn_timer: Option<u8>,
    pub sound: Option<u32>,
}

/// Simple condition opcode: a state-timer test or a player proximity test.
#[derive(Deserialize, Debug, Clone, Copy, Default)]
pub struct CondSpec {
    pub timer_eq: Option<u32>,
    /// (dx, dy) range; dy None = any height.
    pub near: Option<(i32, Option<i32>)>,
    #[serde(default)]
    pub negate: bool,
    pub side: Option<Side>,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

/// Structured opcode table (`objects/opcodes.ops.json`); other opcodes are implemented in code.
#[derive(Asset, TypePath, Deserialize, Debug)]
pub struct OpcodeTable {
    pub init: HashMap<String, InitPreset>,
    pub cond: HashMap<String, CondSpec>,
    pub special: HashMap<String, String>,
    /// ROM data tables used by object scripts (motion paths etc.), keyed by ROM address ("0x180BC").
    #[serde(default)]
    pub tables: HashMap<String, Vec<i32>>,
}

impl JsonAsset for OpcodeTable {}

/// Block attribute values (per-room metatile attribute table).
pub mod attr {
    pub const EMPTY: u8 = 0x00;
    pub const SOLID: u8 = 0x80;
    pub fn is_solid(a: u8) -> bool {
        a & 0x80 != 0
    }
}

// ---------------------------------------------------------------------------
// UI
// ---------------------------------------------------------------------------

/// 8x8 bitmap font: glyph key ("0x41" byte codes or names like "BAR_3") → cell position.
#[derive(Asset, TypePath, Deserialize, Debug)]
pub struct FontDef {
    pub image: String,
    pub cell: u32,
    pub glyphs: HashMap<String, [u32; 2]>,
    #[serde(skip)]
    pub image_handle: Handle<Image>,
}

impl JsonAsset for FontDef {
    fn resolve(&mut self, ctx: &mut LoadContext) {
        let dir = ctx.path().path().parent().map(|p| p.to_path_buf()).unwrap_or_default();
        self.image_handle = ctx.load(dir.join(&self.image));
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct HudCell {
    pub g: String,
    pub h: bool,
    pub v: bool,
}

#[derive(Deserialize, Debug, Clone, Copy)]
pub struct CellPos {
    pub col: usize,
    pub row: usize,
}

#[derive(Deserialize, Debug, Clone, Copy)]
pub struct PerPlayer<T> {
    pub p1: T,
    pub p2: T,
}

#[derive(Deserialize, Debug, Clone)]
pub struct HudNumber {
    pub name: String,
    pub digits: usize,
    pub p1: CellPos,
    pub p2: CellPos,
}

#[derive(Deserialize, Debug, Clone)]
pub struct HudBars {
    pub hp: PerPlayer<CellPos>,
    pub mp: PerPlayer<CellPos>,
}

/// Status bar layout (`ui/hud.hud.json`).
#[derive(Asset, TypePath, Deserialize, Debug)]
pub struct HudDef {
    pub template: Vec<Vec<HudCell>>,
    pub template_cols: PerPlayer<usize>,
    pub name: PerPlayer<CellPos>,
    pub numbers: Vec<HudNumber>,
    pub bars: HudBars,
    pub no_player: String,
}

impl JsonAsset for HudDef {}

// ---------------------------------------------------------------------------
// Dialogue
// ---------------------------------------------------------------------------

/// One step of a text script (cartridge control codes, see docs/re/ui_rpg.md).
#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "op")]
pub enum Token {
    #[serde(rename = "TEXT")]
    Text { s: String },
    #[serde(rename = "NL")]
    Nl,
    #[serde(rename = "P1_NAME")]
    P1Name,
    #[serde(rename = "P2_NAME")]
    P2Name,
    #[serde(rename = "SPEAKER_NAME")]
    SpeakerName,
    #[serde(rename = "PLAYER_NAME")]
    PlayerName,
    #[serde(rename = "PAGE")]
    Page,
    #[serde(rename = "END")]
    End,
    #[serde(rename = "YESNO")]
    YesNo,
    #[serde(rename = "CONT")]
    Cont,
    #[serde(rename = "NUMBER")]
    Number,
    #[serde(rename = "GOTO")]
    Goto { target: String },
    #[serde(rename = "IF_2P")]
    If2p { target: String },
    #[serde(rename = "IF_FLAG")]
    IfFlag { flag: String, target: String },
    #[serde(rename = "INC_FLAG")]
    IncFlag { flag: String },
    #[serde(rename = "CLR_FLAG")]
    ClrFlag { flag: String },
    #[serde(rename = "SET_FLAG")]
    SetFlag {
        flag: String,
        #[serde(default)]
        value: u8,
    },
    /// HUD: debug cheat active (never set in normal play).
    #[serde(rename = "IF_FF0206")]
    IfCheat { target: String },
    /// HUD: no continues left.
    #[serde(rename = "IF_ZERO_A7A")]
    IfNoLives { target: String },
    #[serde(other)]
    Other,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TextNode {
    pub tokens: Vec<Token>,
    /// Node that follows when this one runs out of tokens without ending.
    pub next: Option<String>,
}

#[derive(Deserialize, Debug, Clone, Copy)]
pub struct WindowShape {
    pub w: usize,
    pub h: usize,
    pub x: i32,
    pub y: i32,
}

/// All game text (`text/dialogue.text.json`).
#[derive(Asset, TypePath, Deserialize, Debug)]
pub struct DialogueDef {
    /// Dialogue id (1-based, as a string) → start node address.
    pub entries: HashMap<String, String>,
    pub nodes: HashMap<String, TextNode>,
    pub windows: Vec<WindowShape>,
}

impl JsonAsset for DialogueDef {}

// ---------------------------------------------------------------------------
// Global tables
// ---------------------------------------------------------------------------

#[derive(Deserialize, Clone, Debug)]
pub struct StairWarp {
    pub dy: i32,
    pub freeze: u32,
    pub iframes: u32,
}

#[derive(Deserialize, Clone, Copy, Debug)]
pub struct LevelStats {
    pub exp: u32,
    pub hp: u32,
    pub mp: u32,
    pub atk: u32,
    pub def: u32,
    pub speed: u32,
    pub spells: u32,
    /// Duration of attack wind-up frames (player +158).
    pub attack_frame: u16,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ClassLevels {
    pub fighter: Vec<LevelStats>,
    pub mage: Vec<LevelStats>,
}

/// Physics and progression tables (`data/physics.data.json`).
#[derive(Asset, TypePath, Deserialize, Debug)]
pub struct GameData {
    /// Height masks for one-way surfaces/slopes: 16 rows, bit c = column c is ground.
    pub surface_masks: Vec<[u16; 16]>,
    pub stairs: Vec<StairWarp>,
    pub levels: ClassLevels,
    /// Scripted warps (boss defeats, ending) keyed by event number (cartridge 0xFF0A74).
    #[serde(default)]
    pub event_warps: HashMap<String, ExitDest>,
    /// Recorded pad streams ({ticks, buttons}).
    #[serde(default)]
    pub demos: HashMap<String, Vec<[u8; 2]>>,
}

impl JsonAsset for GameData {}

pub fn plugin(app: &mut App) {
    app.init_asset::<SpriteSheet>()
        .init_asset::<RoomDef>()
        .init_asset::<GameData>()
        .init_asset::<FontDef>()
        .init_asset::<HudDef>()
        .init_asset::<DialogueDef>()
        .init_asset::<RoomObjects>()
        .init_asset::<OpcodeTable>()
        .register_asset_loader(JsonLoader::<RoomObjects>::new(&["objects.json"]))
        .register_asset_loader(JsonLoader::<OpcodeTable>::new(&["ops.json"]))
        .register_asset_loader(JsonLoader::<DialogueDef>::new(&["text.json"]))
        .register_asset_loader(JsonLoader::<FontDef>::new(&["font.json"]))
        .register_asset_loader(JsonLoader::<HudDef>::new(&["hud.json"]))
        .register_asset_loader(JsonLoader::<GameData>::new(&["data.json"]))
        .register_asset_loader(JsonLoader::<SpriteSheet>::new(&["sheet.json"]))
        .register_asset_loader(JsonLoader::<RoomDef>::new(&["room.json"]));
}
