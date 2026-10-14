//! Cadash remake: game plugins and top-level flow.
pub mod anim;
pub mod audio;
pub mod camera;
pub mod collision;
pub mod coords;
pub mod data;
pub mod ending;
pub mod demo;
pub mod dialog;
pub mod frontend;
pub mod hud_message;
pub mod lifecycle;
pub mod magic;
pub mod input;
pub mod objects;
pub mod pause;
pub mod platforms;
pub mod shop;
pub mod player;
pub mod player_look;
pub mod room;
pub mod room_events;
pub mod sprite_palette;
pub mod transition;
pub mod ui;

use bevy::prelude::*;
use crate::controller;
use coords::WorldPos;
use data::{GameData, RoomDef, SpriteSheet};
use player::{CharacterClass, Facing, Stats};
use room::{CurrentRoom, RoomEntity};

#[derive(States, Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppState {
    #[default]
    Loading,
    Sega,
    Title,
    CharSelect,
    NameEntry,
    InGame,
    Ending,
}

/// Handles that must be resident before gameplay starts.
#[derive(Resource)]
pub struct CoreAssets {
    pub fighter: Handle<SpriteSheet>,
    pub mage: Handle<SpriteSheet>,
    /// Every front-end/ending screen image, kept resident so animation frames that are swapped in
    /// by path (SEGA logo shine, character-select frame) are already loaded when first shown;
    /// a not-yet-loaded image is drawn for a frame without its anchor and visibly jumps.
    pub screens: Handle<bevy::asset::LoadedFolder>,
}

/// Global physics/progression tables.
#[derive(Resource)]
pub struct GameDataHandle(pub Handle<GameData>);

/// System parameter giving direct access to the loaded [`GameData`].
#[derive(bevy::ecs::system::SystemParam)]
pub struct GameDataRef<'w> {
    handle: Res<'w, GameDataHandle>,
    assets: Res<'w, Assets<GameData>>,
}

impl GameDataRef<'_> {
    pub fn get(&self) -> Option<&GameData> {
        self.assets.get(&self.handle.0)
    }
}

/// Where a new game starts.
#[derive(Resource, Debug, Clone, Copy)]
pub struct NewGame {
    pub room: usize,
    pub pos: WorldPos,
    pub facing: Facing,
    /// Player 1's class; player 2 (if any) always plays the other one.
    pub class: CharacterClass,
    /// Number of players (1 or 2, cartridge 0xFF0A40 + 1).
    pub players: usize,
    /// `pos` was given on the command line (otherwise the new-game start points are used).
    pub custom_pos: bool,
    /// Starting HP override (testing).
    pub hp: Option<i32>,
    /// Starting gold (testing).
    pub gold: u32,
    /// Starting equipment indices (ac0, ac2, ac4, ac6, ac8, aca).
    pub gear: player_look::Gear,
}

impl NewGame {
    /// Applies `--room N --x X --y Y --class fighter|mage --facing left|right --players 1|2` overrides
    /// (handy for testing and for comparing against the original). With `--players 2`, `--class`
    /// chooses player 1's class and player 2 gets the other.
    pub fn from_args() -> Self {
        let mut g = Self::default();
        let args: Vec<String> = std::env::args().collect();
        let get = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
        if let Some(v) = get("--room").and_then(|v| v.parse().ok()) {
            g.room = v;
        }
        if let Some(v) = get("--x").and_then(|v| v.parse().ok()) {
            g.pos.x = coords::px(v);
            g.custom_pos = true;
        }
        if let Some(v) = get("--y").and_then(|v| v.parse().ok()) {
            g.pos.y = coords::px(v);
            g.custom_pos = true;
        }
        if let Some(v) = get("--players").and_then(|v| v.parse::<usize>().ok()) {
            g.players = v.clamp(1, controller::PORTS);
        }
        match get("--class").as_deref() {
            Some("mage") => g.class = CharacterClass::Mage,
            Some("fighter") => g.class = CharacterClass::Fighter,
            _ => {}
        }
        g.hp = get("--hp").and_then(|v| v.parse().ok());
        g.gold = get("--gold").and_then(|v| v.parse().ok()).unwrap_or(0);
        if let Some(v) = get("--gear") {
            let n: Vec<usize> = v.split(',').filter_map(|x| x.parse().ok()).collect();
            if n.len() == 6 {
                g.gear = player_look::Gear { ac0: n[0], ac2: n[1], ac4: n[2], ac6: n[3], ac8: n[4], aca: n[5] };
            }
        }
        match get("--facing").as_deref() {
            Some("left") => g.facing = Facing::Left,
            Some("right") => g.facing = Facing::Right,
            _ => {}
        }
        g
    }
}

impl NewGame {
    /// Class of player `slot`: player 1 chooses, player 2 plays the other class (0x670C).
    pub fn class_of(&self, slot: usize) -> CharacterClass {
        match (slot, self.class) {
            (0, c) => c,
            (_, CharacterClass::Fighter) => CharacterClass::Mage,
            (_, CharacterClass::Mage) => CharacterClass::Fighter,
        }
    }
    /// Start position of player `slot`. New game in two-player mode (0x907C/0x9114): P1 at raw
    /// x 0x130, P2 at 0x108, i.e. P1 20 px right of the one-player spot and P2 40 px left of P1.
    pub fn start_pos(&self, slot: usize) -> WorldPos {
        let mut p = self.pos;
        if self.players > 1 {
            if !self.custom_pos && self.room == 0 {
                p.x += coords::px(0x130 - 0x11C);
            }
            p.x -= coords::px(0x130 - 0x108) * slot as i32;
        }
        p
    }
}

impl Default for NewGame {
    fn default() -> Self {
        Self {
            room: 0,
            pos: WorldPos::from_px(1020, 368),
            facing: Facing::Right,
            class: CharacterClass::Fighter,
            players: 1,
            custom_pos: false,
            hp: None,
            gold: 0,
            gear: player_look::Gear::default(),
        }
    }
}

pub struct CadashPlugin;

impl Plugin for CadashPlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<AppState>()
            .insert_resource(NewGame::from_args())
            .add_plugins((data::plugin, input::plugin, anim::plugin, player::plugin, camera::plugin, ui::plugin, dialog::plugin))
            .add_plugins((room::plugin, transition::plugin, frontend::plugin, objects::plugin, hud_message::plugin))
            .add_plugins((lifecycle::plugin, player_look::plugin, pause::plugin, shop::plugin, ending::plugin, magic::plugin))
            .add_plugins((room_events::plugin, demo::plugin, sprite_palette::plugin, platforms::plugin, audio::plugin))
            .add_systems(Startup, start_loading)
            .add_systems(Update, wait_for_assets.run_if(in_state(AppState::Loading)))
            .add_systems(OnEnter(AppState::InGame), start_game);
        #[cfg(feature = "dev-mcp")]
        app.add_systems(FixedUpdate, publish_debug.in_set(crate::GameSystems::Finish));
    }
}

#[cfg(feature = "dev-mcp")]
fn publish_debug(
    mut commands: Commands,
    cam: Res<camera::CameraPos>,
    room: Option<Res<CurrentRoom>>,
    q: Query<(&WorldPos, &coords::Velocity, &player::Player, &Stats, &collision::Contacts)>,
    objs: Query<(&objects::Obj, &WorldPos)>,
    lives: Res<lifecycle::Lives>,
    sound: (Res<audio::Music>, Res<audio::SoundLog>),
) {
    let mut list: Vec<_> = objs
        .iter()
        .map(|(o, p)| serde_json::json!({"slot": o.slot, "type": o.ty, "state": o.state, "hp": o.hp, "x": p.x as f64 / 65536.0, "y": p.y as f64 / 65536.0}))
        .collect();
    list.sort_by_key(|v| v["slot"].as_u64());
    let mut players: Vec<_> = q
        .iter()
        .map(|(p, v, pl, s, c)| {
            (pl.slot, serde_json::json!({
                "slot": pl.slot,
                "x": p.x as f64 / 65536.0, "y": p.y as f64 / 65536.0,
                "vx": v.x as f64 / 65536.0, "vy": v.y as f64 / 65536.0,
                "state": format!("{:?}", pl.state), "action": format!("{:?}", pl.action),
                "facing": format!("{:?}", pl.facing), "hp": s.hp, "mp": s.mp, "ground": c.ground, "on_surface": c.on_surface,
                "level": s.level + 1, "exp": s.exp, "gold": s.gold, "herbs": s.herbs,
            }))
        })
        .collect();
    players.sort_by_key(|(slot, _)| *slot);
    let players: Vec<_> = players.into_iter().map(|(_, v)| v).collect();
    commands.insert_resource(crate::dev_mcp::DebugInfo(serde_json::json!({
        "room": room.map(|r| r.index), "camera": [cam.x, cam.y], "player": players.first(), "players": players,
        "lives": lives.0, "objects": list,
        "music": sound.0.current, "sounds": sound.1.0,
    })));
}

fn start_loading(mut commands: Commands, server: Res<AssetServer>, new_game: Res<NewGame>) {
    // Fresh script globals (boss flag, events, music timer) for a new game.
    commands.insert_resource(objects::ops::Effects::default());
    commands.insert_resource(CoreAssets {
        fighter: server.load(CharacterClass::Fighter.sheet_path()),
        mage: server.load(CharacterClass::Mage.sheet_path()),
        screens: server.load_folder("screens"),
    });
    commands.insert_resource(GameDataHandle(server.load("data/physics.data.json")));
    commands.insert_resource(CurrentRoom {
        index: new_game.room,
        def: server.load(room::room_path(new_game.room)),
    });
}

fn wait_for_assets(
    server: Res<AssetServer>,
    core: Res<CoreAssets>,
    data: Res<GameDataHandle>,
    room: Res<CurrentRoom>,
    ui_assets: Res<ui::UiAssets>,
    mut next: ResMut<NextState<AppState>>,
) {
    if ui::ui_loaded(&server, &ui_assets)
        && server.is_loaded_with_dependencies(&core.mage)
        && server.is_loaded_with_dependencies(&core.fighter)
        && server.is_loaded_with_dependencies(&core.screens)
        && server.is_loaded_with_dependencies(&room.def)
        && server.is_loaded_with_dependencies(&data.0)
    {
        // Command-line placement (`--x/--y/--room`) or `--skip-intro` go straight to gameplay.
        let skip = std::env::args().any(|a| matches!(a.as_str(), "--skip-intro" | "--x" | "--y" | "--room"));
        next.set(if skip { AppState::InGame } else { AppState::Sega });
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn start_game(
    mut commands: Commands,
    core: Res<CoreAssets>,
    room: Res<CurrentRoom>,
    rooms: Res<Assets<RoomDef>>,
    new_game: Res<NewGame>,
    data: Res<GameDataHandle>,
    datas: Res<Assets<GameData>>,
    mut cam: ResMut<camera::CameraPos>,
    mut dialog: ResMut<dialog::Dialog>,
    mut cycles: ResMut<room::CycleState>,
    mut fade: ResMut<transition::ScreenFade>,
    mut obj_world: ResMut<objects::ObjectWorld>,
    session: (
        ResMut<player::Party>,
        ResMut<lifecycle::Lives>,
        ResMut<dialog::PartyNames>,
        ResMut<hud_message::HudMessages>,
        Res<demo::AttractDemo>,
        MessageWriter<audio::PlaySound>,
    ),
    roots: Query<Entity, With<ui::ScreenRoot>>,
    old: Query<Entity, With<RoomEntity>>,
    old_players: Query<Entity, With<player::Player>>,
) {
    let (mut party, mut lives, mut names, mut hud, demo, mut sound) = session;
    fade.0 = 1.0;
    commands.insert_resource(transition::FadeTo { target: 0.0, ticks: transition::FADE_TICKS });
    party.count = new_game.players;
    *lives = lifecycle::Lives::for_party(new_game.players);
    hud.clear();
    if new_game.players > 1 && names.p2.is_none() {
        // Skipped name entry: the cartridge's default name for an empty entry.
        names.p2 = Some(match new_game.class_of(1) {
            CharacterClass::Fighter => "FIG".into(),
            CharacterClass::Mage => "MAG".into(),
        });
    } else if new_game.players == 1 {
        names.p2 = None;
    }
    // The king greets a new party (dialogue 1, NPC window).
    if new_game.room == 0 && !std::env::args().any(|a| a == "--no-intro") {
        dialog.open(1, 0);
    }
    room::despawn_room(&mut commands, &old);
    for e in &old_players {
        commands.entity(e).despawn();
    }
    let data = datas.get(&data.0).expect("game data loaded");
    // New game: the castle song (0x4628); a `--room N` start plays that room's song. The attract
    // demo starts its own.
    if !demo.active {
        if let Some(def) = rooms.get(&room.def) {
            sound.write(audio::PlaySound(if new_game.room == 0 { 0x83 } else { def.music }));
        }
    }
    let mut positions: Vec<WorldPos> = (0..new_game.players).map(|slot| new_game.start_pos(slot)).collect();
    if let Some(def) = rooms.get(&room.def) {
        // `--room N` without `--x/--y`: the default position belongs to room 0, so find ground.
        if new_game.room != 0 && !new_game.custom_pos {
            for p in &mut positions {
                if let Some((x, y)) = collision::standing_spot(def, p.px_x(), 48) {
                    *p = WorldPos { x: coords::px(x), y: coords::px(y) };
                }
            }
        }
        // The camera starts centred on the party.
        let n = positions.len() as i32;
        let centre = WorldPos {
            x: positions.iter().map(|p| p.x).sum::<i32>() / n,
            y: positions.iter().map(|p| p.y).sum::<i32>() / n,
        };
        camera::snap_to(&mut cam, def, &centre);
        if let Some(root) = room::screen_root(&roots) {
            room::spawn_room_layers(&mut commands, def, root, &cam, &mut cycles);
        }
    }
    for (slot, pos) in positions.into_iter().enumerate() {
        let class = new_game.class_of(slot);
        let levels = match class {
            CharacterClass::Fighter => &data.levels.fighter,
            CharacterClass::Mage => &data.levels.mage,
        };
        let mut stats = Stats::from_level(0, &levels[0]);
        if let Some(hp) = new_game.hp {
            stats.hp = hp;
        }
        stats.gold = new_game.gold;
        let sheet = match class {
            CharacterClass::Fighter => core.fighter.clone(),
            CharacterClass::Mage => core.mage.clone(),
        };
        let pe = player::spawn_player(&mut commands, slot, sheet, class, stats, pos, new_game.facing);
        commands.entity(pe).insert(new_game.gear);
    }
    objects::clear_objects(&mut obj_world);
}
