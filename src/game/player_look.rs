//! The player's appearance follows their equipment: equipment changes both the sprite tiles
//! (weapon, armour, helmet/shield, sword variants) and palette lines 0–1. Atlases are exported
//! palette-indexed per tile variant and coloured here with the cartridge's palette builder (0xC364).
use super::{
    anim::SpriteAnimator,
    data::{JsonAsset, JsonLoader},
    player::{CharacterClass, Player},
};
use bevy::{
    asset::RenderAssetUsages,
    image::{ImageLoaderSettings, ImageSampler},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use serde::Deserialize;

/// Equipment indices (cartridge RAM 0xFF0AC0..0xFF0ACA).
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Gear {
    pub ac0: usize,
    pub ac2: usize,
    pub ac4: usize,
    pub ac6: usize,
    pub ac8: usize,
    pub aca: usize,
}

impl Gear {
    /// Which pre-rendered tile variant the equipment uses (see extract/player.py).
    fn variant(&self) -> String {
        let w = self.aca & 1;
        let a = self.ac2.min(2);
        let h = usize::from(self.ac4 != 0);
        let s = usize::from(!matches!(self.ac6, 0 | 3));
        format!("{w}{a}{h}{s}")
    }
}

/// Palette builder tables (`data/player.palette.json`).
#[derive(Asset, TypePath, Deserialize, Debug)]
pub struct PlayerPaletteDef {
    pub fixed_1: u16,
    pub b882: Vec<u16>,
    pub b87a: Vec<u16>,
    pub b88a: Vec<u16>,
    pub ac8: Vec<Vec<u16>>,
    pub ac0: Vec<Vec<u16>>,
    pub ac2: Vec<Vec<u16>>,
    pub ac4: Vec<Vec<u16>>,
    pub ac6: Vec<Vec<u16>>,
    pub aca: Vec<Vec<u16>>,
    pub ladder: Vec<u8>,
    /// Green uses slightly different levels.
    pub ladder_g: Vec<u8>,
    /// Skin colours while poisoned (cartridge 0xC2D6).
    pub poison_fighter: Vec<u16>,
    pub poison_mage: Vec<u16>,
}

impl JsonAsset for PlayerPaletteDef {}

impl PlayerPaletteDef {
    /// Palette lines 0 and 1 (32 CRAM words) for the given equipment.
    pub fn build(&self, g: &Gear) -> [u16; 32] {
        let pick = |t: &Vec<Vec<u16>>, i: usize| t.get(i).or(t.first()).cloned().unwrap_or_default();
        let mut p = [0u16; 32];
        p[1] = self.fixed_1;
        p[2..4].copy_from_slice(&self.b882);
        p[4..7].copy_from_slice(&pick(&self.ac8, g.ac8));
        p[7..9].copy_from_slice(&self.b87a);
        p[9..14].copy_from_slice(&pick(&self.ac0, g.ac0));
        p[15] = 0xEEE;
        p[17..19].copy_from_slice(&self.b88a);
        p[19..21].copy_from_slice(&pick(&self.ac2, g.ac2));
        p[21..23].copy_from_slice(&pick(&self.ac4, g.ac4));
        p[23..25].copy_from_slice(&pick(&self.ac6, g.ac6));
        p[25..27].copy_from_slice(&pick(&self.aca, g.aca));
        p[31] = 0xEEE;
        p
    }
    /// Applies the poison tint to a built palette.
    pub fn tint_poison(&self, p: &mut [u16; 32], class: CharacterClass, g: &Gear) {
        match class {
            CharacterClass::Fighter => {
                p[7..9].copy_from_slice(&self.poison_fighter);
                if g.ac0 == 0 {
                    p[12..14].copy_from_slice(&self.poison_fighter);
                }
            }
            CharacterClass::Mage => p[2..4].copy_from_slice(&self.poison_mage),
        }
    }
    pub fn rgb(&self, word: u16) -> [u8; 3] {
        let l = |shift: u16| self.ladder[((word >> shift) & 7) as usize];
        [l(1), self.ladder_g[((word >> 5) & 7) as usize], l(9)]
    }
}

#[derive(Resource)]
pub struct PaletteHandle(pub Handle<PlayerPaletteDef>);

/// Coloured atlas currently used by a player (overrides the sheet's default image).
#[derive(Component, Debug, Clone)]
pub struct SpriteImageOverride(pub Handle<Image>);

/// Which (gear, indexed atlas) the current override was built from.
#[derive(Component, Debug, Clone)]
pub struct LookState {
    pub built_for: Option<Gear>,
    pub indexed: Handle<Image>,
    pub variant: String,
    pub poisoned: bool,
}

fn indexed_path(class: CharacterClass, variant: &str) -> String {
    let base = match class {
        CharacterClass::Fighter => "fighter",
        CharacterClass::Mage => "mage",
    };
    format!("sprites/player/{base}_eq{variant}.png")
}

fn load_indexed(server: &AssetServer, path: String) -> Handle<Image> {
    // Keep pixel data on the CPU so it can be recoloured; no filtering.
    server
        .load_builder()
        .with_settings(|s: &mut ImageLoaderSettings| {
            s.asset_usage = RenderAssetUsages::MAIN_WORLD | RenderAssetUsages::RENDER_WORLD;
            s.sampler = ImageSampler::nearest();
            s.is_srgb = false;
        })
        .load(path)
}

#[allow(clippy::type_complexity)]
fn update_look(
    mut commands: Commands,
    server: Res<AssetServer>,
    pal: Res<PaletteHandle>,
    pals: Res<Assets<PlayerPaletteDef>>,
    mut images: ResMut<Assets<Image>>,
    mut q: Query<(Entity, &CharacterClass, &Gear, &Player, Option<&mut LookState>)>,
) {
    let Some(pal) = pals.get(&pal.0) else { return };
    for (e, class, gear, player, state) in &mut q {
        let variant = gear.variant();
        let Some(mut state) = state else {
            commands.entity(e).insert(LookState {
                built_for: None,
                indexed: load_indexed(&server, indexed_path(*class, &variant)),
                variant,
                poisoned: false,
            });
            continue;
        };
        if state.variant != variant {
            state.variant = variant.clone();
            state.indexed = load_indexed(&server, indexed_path(*class, &variant));
            state.built_for = None;
        }
        if state.poisoned != player.poisoned {
            state.poisoned = player.poisoned;
            state.built_for = None;
        }
        if state.built_for == Some(*gear) {
            continue;
        }
        let Some(src) = images.get(&state.indexed) else { continue };
        let Some(data) = src.data.as_ref() else { continue };
        let (w, h) = (src.width(), src.height());
        let bpp = data.len() / (w as usize * h as usize).max(1);
        let mut words = pal.build(gear);
        if player.poisoned {
            pal.tint_poison(&mut words, *class, gear);
        }
        let mut out = vec![0u8; (w * h * 4) as usize];
        for i in 0..(w * h) as usize {
            let idx = data[i * bpp] as usize;
            if idx == 0 || idx & 15 == 0 {
                continue;
            }
            let [r, g, b] = pal.rgb(words[idx.min(31)]);
            out[i * 4..i * 4 + 4].copy_from_slice(&[r, g, b, 255]);
        }
        let mut img = Image::new(
            Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            TextureDimension::D2,
            out,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::RENDER_WORLD,
        );
        img.sampler = ImageSampler::nearest();
        let handle = images.add(img);
        commands.entity(e).insert(SpriteImageOverride(handle));
        state.built_for = Some(*gear);
    }
}

fn load_palette(mut commands: Commands, server: Res<AssetServer>) {
    commands.insert_resource(PaletteHandle(server.load("data/player.palette.json")));
}

/// Applies image overrides after the generic sprite sync.
pub fn apply_override(mut q: Query<(&SpriteImageOverride, &mut Sprite), With<SpriteAnimator>>) {
    for (o, mut s) in &mut q {
        if s.image != o.0 {
            s.image = o.0.clone();
        }
    }
}

pub fn plugin(app: &mut App) {
    app.init_asset::<PlayerPaletteDef>()
        .register_asset_loader(JsonLoader::<PlayerPaletteDef>::new(&["palette.json"]))
        .add_systems(Startup, load_palette)
        .add_systems(Update, update_look)
        .add_systems(
            PostUpdate,
            apply_override.after(super::anim::sync_sprites).before(TransformSystems::Propagate),
        );
}
