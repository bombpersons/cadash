//! START pauses the game and shows each living player's status window (cartridge 0x2D1AC;
//! layout in `data/shops.ui.json` → `pause_window`, docs/re/shops.md).
use super::{
    AppState, GameDataRef,
    data::{JsonAsset, JsonLoader},
    dialog::{Dialog, PartyNames, WindowId, Windows},
    input::Joypads,
    player::{CharacterClass, Player, PlayerState, Stats},
    transition::Transition,
};
use crate::{GameSystems, controller::Button};
use bevy::prelude::*;
use serde::Deserialize;
use serde_json::Value;

/// Shop/inn/pause data (`data/shops.ui.json`), kept as JSON for the UI code to read.
#[derive(Asset, TypePath, Deserialize, Debug)]
#[serde(transparent)]
pub struct ShopsDef(pub Value);

impl JsonAsset for ShopsDef {}

#[derive(Resource)]
pub struct ShopsHandle(pub Handle<ShopsDef>);

/// Open pause windows (one per living player; empty when not paused).
#[derive(Resource, Default)]
pub struct Pause(pub Vec<WindowId>);

fn load(mut commands: Commands, server: Res<AssetServer>) {
    commands.insert_resource(ShopsHandle(server.load("data/shops.ui.json")));
}

/// Right-aligned number with blank leading zeros.
fn number(v: u32, len: usize) -> String {
    let s = v.to_string();
    let s = if s.len() > len { s[s.len() - len..].to_string() } else { s };
    format!("{s:>len$}")
}

#[allow(clippy::too_many_arguments)]
fn toggle_pause(
    pads: Res<Joypads>,
    mut pause: ResMut<Pause>,
    mut windows: ResMut<Windows>,
    dialog: Res<Dialog>,
    transition: Option<Res<Transition>>,
    shops: Res<ShopsHandle>,
    defs: Res<Assets<ShopsDef>>,
    names: Res<PartyNames>,
    data: GameDataRef,
    players: Query<(&Player, &Stats, &CharacterClass)>,
) {
    // START of a living player toggles (cartridge 0x2B5C: P1 or P2 with state < 8).
    let start = players.iter().any(|(p, ..)| p.state < PlayerState::Dying && pads.get(p.slot).pressed(Button::Start));
    if !start {
        return;
    }
    if !pause.0.is_empty() {
        for id in pause.0.drain(..) {
            windows.close(id);
        }
        return;
    }
    // No pausing inside conversations, shops or transitions.
    if dialog.is_open() || windows.any_open() || transition.is_some() {
        return;
    }
    let Some(def) = defs.get(&shops.0) else { return };
    let mut party: Vec<_> = players.iter().filter(|(p, ..)| p.state < PlayerState::Dying).collect();
    party.sort_by_key(|(p, ..)| p.slot);
    for (p, s, class) in party {
        // P1 window shape 13 at (8,8), P2 window 14 at (168,8).
        let id = windows.open(13 + p.slot.min(1));
        let name = if p.slot == 0 { names.p1.clone() } else { names.p2.clone().unwrap_or_default() };
        let w = windows.get_mut(id).unwrap();
        let next_exp = data
            .get()
            .and_then(|d| {
                let levels = match class {
                    CharacterClass::Fighter => &d.levels.fighter,
                    CharacterClass::Mage => &d.levels.mage,
                };
                levels.get(s.level + 1).map(|l| l.exp)
            })
            .unwrap_or(65535);
        let cells = def.0["pause_window"]["cells"].as_array().cloned().unwrap_or_default();
        for c in cells {
            let (col, row) = (c["cell"][0].as_u64().unwrap_or(0) as usize, c["cell"][1].as_u64().unwrap_or(0) as usize);
            if let Some(text) = c["text"].as_str() {
                w.put_text(col, row, &text.replace('\u{d7}', "×"));
                continue;
            }
            let len = c["len"].as_u64().unwrap_or(1) as usize;
            let value = match c["field"].as_str().unwrap_or("") {
                "name" => {
                    w.put_text(col, row, &name);
                    continue;
                }
                "level" => s.level as u32 + 1,
                "gold" => s.gold,
                "exp" => s.exp,
                "next_exp" => next_exp,
                "str" => s.atk as u32,
                "ac" => s.def as u32,
                "agl" => s.speed as u32,
                "herbs" => s.herbs,
                "antidotes" => s.antidotes,
                "elixirs" => s.elixirs,
                _ => continue,
            };
            w.put_text(col, row, &number(value, len));
        }
        pause.0.push(id);
    }
}

pub fn plugin(app: &mut App) {
    app.init_asset::<ShopsDef>()
        .register_asset_loader(JsonLoader::<ShopsDef>::new(&["ui.json"]))
        .init_resource::<Pause>()
        .add_systems(Startup, load)
        .add_systems(
            FixedUpdate,
            toggle_pause
                .in_set(GameSystems::Game)
                .before(super::player::tick)
                .run_if(in_state(AppState::InGame))
                .run_if(super::room_events::world_running)
                .run_if(super::demo::not_in_demo),
        );
}
