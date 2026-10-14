//! Screen-space UI drawn with the cartridge's 8x8 font: the status bar (HUD) and text cells.
//!
//! UI lives under a [`ScreenRoot`] parented to the game camera, so its children use plain
//! screen coordinates (x right, y down, origin top-left of the 320x224 screen).
use super::{
    AppState,
    coords::z,
    data::{DialogueDef, FontDef, HudDef},
    player::{Player, PlayerState, Stats},
};
use crate::screen::GameCamera;
use bevy::{prelude::*, sprite::Anchor};

/// Parent of all screen-space UI.
#[derive(Component)]
pub struct ScreenRoot;

#[derive(Resource)]
pub struct UiAssets {
    pub font: Handle<FontDef>,
    pub hud: Handle<HudDef>,
    pub dialogue: Handle<DialogueDef>,
}

/// Translates ASCII text to the cartridge's glyph bytes (0x30..0x5F).
pub fn glyph_byte(c: char) -> u8 {
    match c {
        '0'..='9' | 'A'..='Z' => c as u8,
        'a'..='z' => c.to_ascii_uppercase() as u8,
        ',' => 0x3B,
        '.' => 0x3C,
        '!' => 0x3D,
        '\'' => 0x3E,
        '?' => 0x3F,
        '/' => 0x5B,
        '×' => 0x5C,
        _ => 0x3A,
    }
}

/// A fixed grid of 8x8 cells in screen space.
#[derive(Component)]
pub struct CellGrid {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Entity>,
}

/// Cell contents: glyph key in the font (e.g. "0x41" or "BAR_3") and flips.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cell {
    pub glyph: String,
    pub hflip: bool,
    pub vflip: bool,
}

impl Cell {
    pub fn byte(b: u8) -> Self {
        Self { glyph: format!("0x{b:02X}"), hflip: false, vflip: false }
    }
    pub fn named(name: &str) -> Self {
        Self { glyph: name.to_owned(), hflip: false, vflip: false }
    }
    pub fn blank() -> Self {
        Self::byte(0x3A)
    }
}

pub fn set_cell(font: &FontDef, sprite: &mut Sprite, cell: &Cell) {
    let Some(&[x, y]) = font.glyphs.get(&cell.glyph) else { return };
    let s = font.cell as f32;
    sprite.image = font.image_handle.clone();
    sprite.rect = Some(Rect::new(x as f32, y as f32, x as f32 + s, y as f32 + s));
    sprite.flip_x = cell.hflip;
    sprite.flip_y = cell.vflip;
}

/// Spawns a grid of cells at screen position (x, y).
pub fn spawn_grid(commands: &mut Commands, root: Entity, x: i32, y: i32, cols: usize, rows: usize, depth: f32) -> Entity {
    let grid = commands
        .spawn((
            Transform::from_xyz(x as f32, -(y as f32), depth),
            Visibility::Inherited,
            ChildOf(root),
        ))
        .id();
    let mut cells = Vec::with_capacity(cols * rows);
    for r in 0..rows {
        for c in 0..cols {
            cells.push(
                commands
                    .spawn((
                        Sprite::default(),
                        Anchor::TOP_LEFT,
                        Transform::from_xyz((c * 8) as f32, -((r * 8) as f32), 0.0),
                        ChildOf(grid),
                    ))
                    .id(),
            );
        }
    }
    commands.entity(grid).insert(CellGrid { cols, rows, cells });
    grid
}

pub fn write_text(font: &FontDef, sprites: &mut Query<&mut Sprite>, grid: &CellGrid, col: usize, row: usize, text: &str) {
    for (i, ch) in text.chars().enumerate() {
        let c = col + i;
        if c >= grid.cols || row >= grid.rows {
            break;
        }
        if let Ok(mut s) = sprites.get_mut(grid.cells[row * grid.cols + c]) {
            set_cell(font, &mut s, &Cell::byte(glyph_byte(ch)));
        }
    }
}

pub fn put(font: &FontDef, sprites: &mut Query<&mut Sprite>, grid: &CellGrid, col: usize, row: usize, cell: &Cell) {
    if col < grid.cols && row < grid.rows {
        if let Ok(mut s) = sprites.get_mut(grid.cells[row * grid.cols + col]) {
            set_cell(font, &mut s, cell);
        }
    }
}

// ---------------------------------------------------------------------------
// HUD
// ---------------------------------------------------------------------------

/// The status bar along the bottom four rows of the screen.
#[derive(Component)]
pub struct Hud;

/// Player name shown in the HUD (three letters, entered on the name screen).
#[derive(Component, Clone, Debug)]
pub struct PlayerName(pub String);

pub fn setup_screen_root(mut commands: Commands, cam: Query<Entity, With<GameCamera>>, existing: Query<(), With<ScreenRoot>>) {
    if !existing.is_empty() {
        return;
    }
    let Ok(cam) = cam.single() else { return };
    commands.spawn((
        ScreenRoot,
        Transform::from_xyz(-160.0, 112.0, 0.0),
        Visibility::Inherited,
        ChildOf(cam),
    ));
}

fn spawn_hud(mut commands: Commands, root: Query<Entity, With<ScreenRoot>>, existing: Query<(), With<Hud>>) {
    if !existing.is_empty() {
        return;
    }
    let Ok(root) = root.single() else { return };
    // HUD rows 24..27 span the full 40-cell screen width.
    let grid = spawn_grid(&mut commands, root, 0, 192, 40, 4, z::HUD);
    commands.entity(grid).insert(Hud);
}

fn hp_bar(value: i32) -> [Cell; 8] {
    // px = ceil(value / 4); full cells first, then one partial cell, rest blank.
    let px = ((value.max(0) + 3) / 4) as usize;
    std::array::from_fn(|i| {
        let filled = px.saturating_sub(i * 8).min(8);
        match filled {
            0 => Cell::blank(),
            n => Cell::named(&format!("BAR_{n}")),
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn draw_hud(
    ui: Res<UiAssets>,
    fonts: Res<Assets<FontDef>>,
    huds: Res<Assets<HudDef>>,
    hud: Query<&CellGrid, With<Hud>>,
    players: Query<(&Stats, &Player, Option<&PlayerName>)>,
    messages: Res<super::hud_message::HudMessages>,
    names: Res<super::dialog::PartyNames>,
    mut sprites: Query<&mut Sprite>,
) {
    let (Some(font), Some(def), Ok(grid)) = (fonts.get(&ui.font), huds.get(&ui.hud), hud.single()) else { return };
    // Background: every HUD cell is a solid black space, then one template per player
    // (P1 at template column 1, P2 at column 20; cartridge 0x8992).
    for r in 0..grid.rows {
        for c in 0..grid.cols {
            put(font, &mut sprites, grid, c, r, &Cell::blank());
        }
    }
    let mut any = false;
    let mut second = false;
    for (stats, player, name) in &players {
        let slot = player.slot;
        if slot > 1 {
            continue;
        }
        any = true;
        second |= slot == 1;
        let pick = |p: &super::data::CellPos, q: &super::data::CellPos| if slot == 0 { (p.col, p.row) } else { (q.col, q.row) };
        let tcol = if slot == 0 { def.template_cols.p1 } else { def.template_cols.p2 };
        for (r, row) in def.template.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                put(font, &mut sprites, grid, tcol + c, r, &Cell { glyph: cell.g.clone(), hflip: cell.h, vflip: cell.v });
            }
        }
        let fallback = if slot == 0 { names.p1.clone() } else { names.p2.clone().unwrap_or_default() };
        let name = name.map_or(fallback, |n| n.0.clone());
        let (ncol, _) = pick(&def.name.p1, &def.name.p2);
        write_text(font, &mut sprites, grid, ncol, 0, &name);
        if let Some(f) = def.numbers.iter().find(|f| f.name == "level") {
            let text = format!("{:>width$}", stats.level + 1, width = f.digits);
            let (c, r) = pick(&f.p1, &f.p2);
            write_text(font, &mut sprites, grid, c, r - 24, &text);
        }
        // A status message replaces the HP/MP rows; a finished one only stays for a dead player.
        let dead = player.state >= PlayerState::Dying;
        let msg = messages.cells_for(slot).filter(|_| dead || !messages.holding(slot));
        if let Some(cells) = msg {
            for (r, row) in cells.iter().enumerate() {
                for (c, &b) in row.iter().enumerate() {
                    put(font, &mut sprites, grid, tcol + 1 + c, 1 + r, &Cell::byte(b));
                }
            }
        } else if !dead {
            for f in &def.numbers {
                let value = match f.name.as_str() {
                    "level" => continue,
                    "hp" => stats.hp,
                    "hp_max" => stats.max_hp,
                    "mp" => stats.mp,
                    "mp_max" => stats.max_mp,
                    _ => continue,
                };
                let text = format!("{:>width$}", value, width = f.digits);
                let (c, r) = pick(&f.p1, &f.p2);
                write_text(font, &mut sprites, grid, c, r - 24, &text);
            }
            for (bar, value) in [(&def.bars.hp, stats.hp), (&def.bars.mp, stats.mp)] {
                let (bc, br) = pick(&bar.p1, &bar.p2);
                for (i, cell) in hp_bar(value).iter().enumerate() {
                    put(font, &mut sprites, grid, bc + i, br - 24, cell);
                }
            }
        }
    }
    if any && !second {
        // No second player: "NO PLAYER" in the right half.
        write_text(font, &mut sprites, grid, def.template_cols.p2 + 6, 1, &def.no_player);
    }
}

fn load_ui(mut commands: Commands, server: Res<AssetServer>) {
    commands.insert_resource(UiAssets {
        font: server.load("ui/font.font.json"),
        hud: server.load("ui/hud.hud.json"),
        dialogue: server.load("text/dialogue.text.json"),
    });
}

pub fn ui_loaded(server: &AssetServer, ui: &UiAssets) -> bool {
    server.is_loaded_with_dependencies(&ui.font)
        && server.is_loaded_with_dependencies(&ui.hud)
        && server.is_loaded_with_dependencies(&ui.dialogue)
}

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, load_ui)
        .add_systems(PostStartup, setup_screen_root)
        .add_systems(OnEnter(AppState::InGame), spawn_hud)
        .add_systems(Update, draw_hud.run_if(in_state(AppState::InGame)));
}
