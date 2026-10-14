//! Inns, armories and apothecaries (docs/re/shops.md; data `data/shops.ui.json`).
//!
//! A shop is a small state machine over several windows: the customer's LV/GOLD box, greeting text,
//! YES/NO box, the goods list (icons, prices, cursor, typed descriptions), messages and goodbye.
//! The whole game is frozen while a shop is open.
use super::{
    AppState,
    coords::z,
    dialog::{RunPhase, TextRun, WindowId, Windows},
    input::{Joypad, Joypads},
    lifecycle::Lives,
    pause::{ShopsDef, ShopsHandle},
    player::{CharacterClass, Player, PlayerState, Stats},
    player_look::Gear,
    room::CurrentRoom,
    transition::{FADE_TICKS, FadeTo, ScreenFade},
    ui::ScreenRoot,
};
use super::audio::{PlaySound, SetMusicTimer};
use crate::{GameSystems, controller::Button};
use bevy::{prelude::*, sprite::Anchor};
use serde_json::Value;

/// Request from NPC interaction to open the shop of the current room: the slot of the player who
/// talked to the shopkeeper (the customer, who pays and controls the menus).
#[derive(Resource, Default)]
pub struct ShopRequest(pub Option<usize>);

/// Price of the next extra life (cartridge 0xFF0A78): 800, doubling up to 6400.
#[derive(Resource)]
pub struct ExtraLifePrice(pub u32);

impl Default for ExtraLifePrice {
    fn default() -> Self {
        Self(800)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Inn,
    Armory,
    Apothecary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum After {
    /// Greeting answered.
    Greeting,
    /// "Something else?" answered.
    Again,
    /// Inn: stay tonight?
    Inn,
}

#[derive(Debug, Clone, PartialEq)]
enum Step {
    /// Text window typing; on YES/NO stop open the YES/NO box.
    Text { win: WindowId, then: After },
    YesNo { text: WindowId, yn: WindowId, yes: bool, then: After },
    /// Waiting for an auto-closing message, then go on.
    Message { win: WindowId, next: Box<Step> },
    List,
    /// Two players: "WHO IS THIS FOR?" with a cursor on the names (0x32800..).
    Who { win: WindowId, choice: usize },
    InnSleep { t: u32 },
    /// Ask "would you like something else?".
    OpenAgain,
    Bye { win: WindowId },
    Done,
}

#[derive(Resource)]
pub struct Shop {
    kind: Kind,
    room: String,
    lvgold: WindowId,
    list: Option<WindowId>,
    slot: usize,
    step: Step,
    price: u32,
    /// Slot of the player who talked to the shopkeeper: pays, uses their pad and cursor colour.
    customer: usize,
}

/// Item icons, prices and the cursor drawn over the goods list.
#[derive(Component)]
struct ShopSprite;

fn kind_of(v: &Value) -> Kind {
    match v["kind"].as_str() {
        Some("inn") => Kind::Inn,
        Some("armory") => Kind::Armory,
        _ => Kind::Apothecary,
    }
}

fn number(v: u32, len: usize) -> String {
    format!("{v:>len$}")
}

/// Opens a text window with shop typing rules: 2 ticks per glyph, no B speed-up, endings
/// continue after 81 ticks.
fn shop_text(windows: &mut Windows, shape: usize, id: u32, number_value: u32) -> WindowId {
    let win = windows.open_text(shape, id);
    if let Some(t) = windows.get_mut(win).and_then(|w| w.text.as_mut()) {
        t.b_speedup = false;
        t.auto_end = Some(0x51);
        t.auto_page = Some(0x3D);
        t.number = number_value;
    }
    win
}

fn open_lvgold(windows: &mut Windows, name: &str, s: &Stats) -> WindowId {
    let id = windows.open(8);
    redraw_lvgold(windows, id, name, s);
    id
}

fn redraw_lvgold(windows: &mut Windows, id: WindowId, name: &str, s: &Stats) {
    if let Some(w) = windows.get_mut(id) {
        w.put_text(2, 0, name);
        w.put_text(7, 0, "LV");
        w.put_text(9, 0, &number(s.level as u32 + 1, 2));
        w.put_text(2, 2, "GOLD");
        w.put_text(7, 2, &number(s.gold, 5));
    }
}

#[allow(clippy::too_many_arguments)]
fn start_shop(
    mut commands: Commands,
    mut req: ResMut<ShopRequest>,
    room: Option<Res<CurrentRoom>>,
    shops: Res<ShopsHandle>,
    defs: Res<Assets<ShopsDef>>,
    mut windows: ResMut<Windows>,
    names: Res<super::dialog::PartyNames>,
    players: Query<(&Player, &Stats)>,
) {
    let Some(customer) = req.0.take() else { return };
    let (Some(room), Some(def)) = (room, defs.get(&shops.0)) else { return };
    let key = room.index.to_string();
    let Some(shop) = def.0["shops"].get(&key) else { return };
    let Some((_, stats)) = players.iter().find(|(p, _)| p.slot == customer) else { return };
    let kind = kind_of(shop);
    let lvgold = open_lvgold(&mut windows, &name_of(&names, customer), stats);
    let price = shop["price"].as_u64().unwrap_or(100) as u32;
    let (greet, then) = match kind {
        Kind::Inn => (95, After::Inn),
        Kind::Armory => (100, After::Greeting),
        Kind::Apothecary => (107, After::Greeting),
    };
    let win = shop_text(&mut windows, 9, greet, price);
    commands.insert_resource(Shop { kind, room: key, lvgold, list: None, slot: 0, step: Step::Text { win, then }, price, customer });
}

fn name_of(names: &super::dialog::PartyNames, slot: usize) -> String {
    if slot == 0 { names.p1.clone() } else { names.p2.clone().unwrap_or_default() }
}

fn texts(kind: Kind) -> (u32, u32, u32) {
    // (bye, no money, -) per kind.
    match kind {
        Kind::Armory => (103, 104, 0),
        Kind::Apothecary => (108, 109, 0),
        Kind::Inn => (98, 97, 0),
    }
}

/// Item → (current value of its target, apply purchase).
fn owned(rec: &Value, s: &Stats, gear: &Gear, lives: &Lives, flags: &super::dialog::StoryFlags) -> u32 {
    match rec["kind"].as_str() {
        Some("consumable") => match rec["counter"].as_str() {
            Some("herbs") => s.herbs,
            Some("antidotes") => s.antidotes,
            Some("bells") => s.bells,
            _ => 0,
        },
        Some("extra_life") => lives.0,
        Some("key_item") => flags.0.get(&0xFF0AD3).copied().unwrap_or(0) as u32,
        Some("equipment") => match rec["slot"].as_str() {
            Some("fighter_weapon") => gear.ac6 as u32,
            Some("fighter_armor") => gear.ac0 as u32,
            Some("fighter_shield") => gear.ac4 as u32,
            Some("fighter_helm") => gear.ac2 as u32,
            Some("mage_wand") => gear.aca as u32,
            Some("mage_robe") => gear.ac8 as u32,
            _ => 0,
        },
        _ => 0,
    }
}

#[allow(clippy::too_many_arguments)]
fn run_shop(
    mut commands: Commands,
    server: Res<AssetServer>,
    pads: Res<Joypads>,
    shop: Option<ResMut<Shop>>,
    shops: Res<ShopsHandle>,
    defs: Res<Assets<ShopsDef>>,
    mut windows: ResMut<Windows>,
    mut lives: ResMut<Lives>,
    mut life_price: ResMut<ExtraLifePrice>,
    mut flags: ResMut<super::dialog::StoryFlags>,
    names: Res<super::dialog::PartyNames>,
    data: super::GameDataRef,
    roots: Query<Entity, With<ScreenRoot>>,
    sprites: Query<Entity, With<ShopSprite>>,
    fade_and_audio: (ResMut<ScreenFade>, MessageWriter<PlaySound>, MessageWriter<SetMusicTimer>),
    mut players: Query<(&Player, &mut Stats, &mut Gear, &CharacterClass)>,
) {
    let (mut fade, mut sound, mut music_timer) = fade_and_audio;
    let Some(mut shop) = shop else { return };
    let Some(def) = defs.get(&shops.0) else { return };
    let pad = pads.get(shop.customer);
    let customer_name = name_of(&names, shop.customer);
    let mut party: Vec<_> = players.iter_mut().collect();
    party.sort_by_key(|(p, ..)| p.slot);
    let Some(ci) = party.iter().position(|(p, ..)| p.slot == shop.customer) else { return };
    let shop_def = def.0["shops"][&shop.room].clone();
    let records = def.0["records"].as_array().cloned().unwrap_or_default();
    let (bye_text, no_money_text, _) = texts(shop.kind);
    // Cursor moves and confirmations click (B4); a payment rings the till (9C, 0x32B26).
    let gold_before = party[ci].1.gold;
    let mut click = false;
    let step = shop.step.clone();
    let buying = matches!(step, Step::List | Step::Who { .. });
    let next = match step {
        Step::Text { win, then } => match windows.get(win).and_then(|w| w.run_phase()) {
            Some(RunPhase::Halted) => {
                let yn = windows.open(5);
                if let Some(w) = windows.get_mut(yn) {
                    w.put_text(2, 1, " YES");
                    w.put_text(2, 2, " NO ");
                }
                Step::YesNo { text: win, yn, yes: true, then }
            }
            _ => step,
        },
        Step::YesNo { text, yn, mut yes, then } => {
            if windows.get(yn).is_some_and(|w| w.is_open()) {
                let before = yes;
                if pad.pressed(Button::Up) {
                    yes = true;
                } else if pad.pressed(Button::Down) {
                    yes = false;
                }
                click |= yes != before;
            }
            let decided = if pad.pressed(Button::B) {
                click |= yes;
                Some(yes)
            } else if pad.pressed(Button::A) || pad.pressed(Button::C) {
                Some(false)
            } else {
                None
            };
            match decided {
                None => Step::YesNo { text, yn, yes, then },
                Some(accept) => {
                    windows.close(yn);
                    windows.close(text);
                    match (then, accept) {
                        (After::Inn, false) => Step::Bye { win: shop_text(&mut windows, 10, bye_text, 0) },
                        (After::Inn, true) => {
                            // Only the customer pays and is healed.
                            let stats = &mut party[ci].1;
                            if stats.gold < shop.price {
                                Step::Bye { win: shop_text(&mut windows, 10, no_money_text, 0) }
                            } else {
                                stats.gold -= shop.price;
                                stats.hp = stats.max_hp;
                                stats.mp = stats.max_mp;
                                redraw_lvgold(&mut windows, shop.lvgold, &customer_name, stats);
                                let win = shop_text(&mut windows, 10, 96, 0);
                                Step::Message { win, next: Box::new(Step::InnSleep { t: 0 }) }
                            }
                        }
                        (After::Greeting, false) | (After::Again, false) => {
                            if let Some(l) = shop.list.take() {
                                windows.close(l);
                            }
                            Step::Bye { win: shop_text(&mut windows, 10, bye_text, 0) }
                        }
                        (After::Greeting, true) => {
                            let l = windows.open(4);
                            shop.list = Some(l);
                            if let Some(w) = windows.get_mut(l) {
                                let mut t = TextRun::new(101, (2, 9), (24, 6));
                                t.interval = 2;
                                w.text = Some(t);
                            }
                            Step::List
                        }
                        (After::Again, true) => {
                            restart_description(&mut windows, &shop, &shop_def);
                            Step::List
                        }
                    }
                }
            }
        }
        Step::OpenAgain => Step::Text { win: shop_text(&mut windows, 6, 102, 0), then: After::Again },
        Step::Message { win, next } => {
            if windows.get(win).and_then(|w| w.run_phase()) == Some(RunPhase::Done) || windows.is_gone(win) {
                windows.close(win);
                *next
            } else {
                Step::Message { win, next }
            }
        }
        Step::InnSleep { t } => {
            // Fade to black, 60 dark ticks, fade back in, then "good morning".
            let t = t + 1;
            if t == 1 {
                commands.insert_resource(FadeTo { target: 1.0, ticks: FADE_TICKS });
            }
            if t == FADE_TICKS + 60 {
                commands.insert_resource(FadeTo { target: 0.0, ticks: FADE_TICKS });
                // Inn jingle; the room music comes back when the timer runs down (0xD7).
                sound.write(PlaySound(0x94));
                music_timer.write(SetMusicTimer(0xD7));
            }
            if t >= 2 * FADE_TICKS + 61 && fade.0 <= 0.0 {
                let win = shop_text(&mut windows, 10, 99, 0);
                Step::Message { win, next: Box::new(Step::Done) }
            } else {
                Step::InnSleep { t }
            }
        }
        Step::List => {
            // Two players (the partner alive): ask who the item is for (0x32800).
            let partner_alive = party.iter().any(|(p, ..)| p.slot != shop.customer && p.state < PlayerState::Dying);
            match list_input(&mut shop, &pad, &shop_def, &mut windows, &mut click) {
                ListInput::Gone => Step::Done,
                ListInput::Stay => Step::List,
                ListInput::Leave => Step::Bye { win: shop_text(&mut windows, 10, bye_text, 0) },
                ListInput::Choose if partner_alive => {
                    clear_description(&mut windows, &shop);
                    let win = shop_text(&mut windows, 6, 105, 0);
                    if let Some(w) = windows.get_mut(win) {
                        w.put_text(9, 4, &names.p1);
                        w.put_text(9, 5, &names.p2.clone().unwrap_or_default());
                    }
                    Step::Who { win, choice: shop.customer }
                }
                ListInput::Choose => {
                    let customer = shop.customer;
                    buy(
                    &mut shop, customer, &shop_def, &records, &mut windows, &mut party, &mut lives, &mut life_price,
                    &mut flags, &customer_name, &data,
                )
                }
            }
        }
        Step::Who { win, mut choice } => {
            let before = choice;
            if pad.pressed(Button::Up) {
                choice = 0;
            } else if pad.pressed(Button::Down) {
                choice = 1;
            }
            click |= choice != before;
            let halted = windows.get(win).and_then(|w| w.run_phase()) == Some(RunPhase::Halted);
            if halted && (pad.pressed(Button::A) || pad.pressed(Button::C)) {
                windows.close(win);
                if let Some(l) = shop.list.take() {
                    windows.close(l);
                }
                Step::Bye { win: shop_text(&mut windows, 10, bye_text, 0) }
            } else if halted && pad.pressed(Button::B) {
                windows.close(win);
                buy(
                    &mut shop, choice, &shop_def, &records, &mut windows, &mut party, &mut lives, &mut life_price,
                    &mut flags, &customer_name, &data,
                )
            } else {
                Step::Who { win, choice }
            }
        }
        Step::Bye { win } => {
            if windows.get(win).and_then(|w| w.run_phase()) == Some(RunPhase::Done) {
                windows.close(win);
                Step::Done
            } else {
                Step::Bye { win }
            }
        }
        Step::Done => {
            windows.close(shop.lvgold);
            if windows.is_gone(shop.lvgold) {
                commands.remove_resource::<Shop>();
            }
            Step::Done
        }
    };
    shop.step = next;
    if click {
        sound.write(PlaySound(0xB4));
    }
    if buying && party.get(ci).is_some_and(|row| row.1.gold < gold_before) {
        sound.write(PlaySound(0x9C));
    }
    let _ = &mut fade;
    draw_list_sprites(&mut commands, &server, &shop, &shop_def, &windows, &roots, &sprites, &life_price);
}

fn restart_description(windows: &mut Windows, shop: &Shop, shop_def: &Value) {
    let Some(l) = shop.list else { return };
    let item = item_in_slot(shop_def, shop.slot);
    let id = item.map_or(101, |i| i["description_text_id"].as_u64().unwrap_or(101) as u32);
    if let Some(t) = windows.get_mut(l).and_then(|w| w.text.as_mut()) {
        t.restart(id);
    }
}

fn item_in_slot(shop_def: &Value, slot: usize) -> Option<&Value> {
    shop_def["items"].as_array()?.iter().find(|i| i["slot"].as_u64() == Some(slot as u64))
}

enum ListInput {
    /// The list window is gone.
    Gone,
    Stay,
    Leave,
    Choose,
}

/// Goods list: cursor moves, A/C leaves, B chooses the item under the cursor.
fn list_input(shop: &mut Shop, pad: &Joypad, shop_def: &Value, windows: &mut Windows, click: &mut bool) -> ListInput {
    let Some(list) = shop.list else { return ListInput::Gone };
    let Some(w) = windows.get(list) else { return ListInput::Gone };
    if !w.is_open() {
        return ListInput::Stay;
    }
    // Input is only read once the opening prompt has been typed.
    let prompt_done = w.text.as_ref().is_some_and(|t| t.start_id != 101 || t.phase == RunPhase::Halted);
    if !prompt_done {
        return ListInput::Stay;
    }
    if w.text.as_ref().is_some_and(|t| t.start_id == 101) {
        restart_description(windows, shop, shop_def);
        return ListInput::Stay;
    }
    let grid: Vec<u8> = shop_def["slot_grid"].as_array().map(|g| g.iter().map(|v| v.as_u64().unwrap_or(255) as u8).collect()).unwrap_or_default();
    let moves = [(Button::Up, -4i32), (Button::Down, 4), (Button::Left, -1), (Button::Right, 1)];
    for (b, d) in moves {
        if pad.pressed(b) {
            // The click sounds even when the move is refused.
            *click = true;
            let target = shop.slot as i32 + d;
            if target >= 0 && grid.get(target as usize).is_some_and(|&g| g != 255) {
                shop.slot = target as usize;
                restart_description(windows, shop, shop_def);
            }
            return ListInput::Stay;
        }
    }
    if pad.pressed(Button::A) || pad.pressed(Button::C) {
        windows.close(list);
        shop.list = None;
        return ListInput::Leave;
    }
    if pad.pressed(Button::B) && item_in_slot(shop_def, shop.slot).is_some() {
        return ListInput::Choose;
    }
    ListInput::Stay
}

/// The description area of the list is cleared before a message window opens.
fn clear_description(windows: &mut Windows, shop: &Shop) {
    if let Some(t) = shop.list.and_then(|l| windows.get_mut(l)).and_then(|w| w.text.as_mut()) {
        t.clear();
        t.phase = RunPhase::Halted;
    }
}

type PartyRow<'a> = (&'a Player, Mut<'a, Stats>, Mut<'a, Gear>, &'a CharacterClass);

/// Buying the selected item for player `recipient` (class/count checks and effects); the customer
/// pays (0x32B26).
#[allow(clippy::too_many_arguments)]
fn buy(
    shop: &mut Shop,
    recipient: usize,
    shop_def: &Value,
    records: &[Value],
    windows: &mut Windows,
    party: &mut [PartyRow],
    lives: &mut Lives,
    life_price: &mut ExtraLifePrice,
    flags: &mut super::dialog::StoryFlags,
    customer_name: &str,
    data: &super::GameDataRef,
) -> Step {
    let Some(item) = item_in_slot(shop_def, shop.slot) else { return Step::List };
    let rec_i = item["record"].as_u64().unwrap_or(0) as usize;
    let Some(rec) = records.get(rec_i) else { return Step::List };
    let Some(ci) = party.iter().position(|(p, ..)| p.slot == shop.customer) else { return Step::Done };
    let ri = party.iter().position(|(p, ..)| p.slot == recipient).unwrap_or(ci);
    let class = *party[ri].3;
    let class_ok = match rec["class"].as_str() {
        Some("fighter") => class == CharacterClass::Fighter,
        Some("mage") => class == CharacterClass::Mage,
        _ => true,
    };
    let limit = rec["tier"].as_u64().or(rec["max_count"].as_u64()).unwrap_or(1) as u32;
    let price = if rec["kind"] == "extra_life" { life_price.0 } else { rec["price"].as_u64().unwrap_or(0) as u32 };
    let (_, no_money, _) = texts(shop.kind);
    let msg = if !class_ok {
        Some(106)
    } else if owned(rec, &party[ri].1, &party[ri].2, lives, flags) >= limit {
        Some(110)
    } else if party[ci].1.gold < price {
        Some(no_money)
    } else {
        party[ci].1.gold -= price;
        let equipment = rec["kind"] == "equipment";
        {
            let (_, stats, gear, _) = &mut party[ri];
            apply_purchase(rec, stats, gear, class, lives, life_price, flags, data);
        }
        if equipment {
            // Equipment slots are global (0xFF0AC0..): every player sees the new gear.
            let gear = *party[ri].2;
            for (_, stats, g, c) in party.iter_mut() {
                **g = gear;
                if let Some(d) = data.get() {
                    super::player::recompute_stats(stats, g, **c, d);
                }
            }
        }
        None
    };
    redraw_lvgold(windows, shop.lvgold, customer_name, &party[ci].1);
    clear_description(windows, shop);
    match msg {
        // Refusals are shown first; "something else?" follows when the message closes.
        Some(m) => Step::Message { win: shop_text(windows, 6, m, 0), next: Box::new(Step::OpenAgain) },
        None => Step::Text { win: shop_text(windows, 6, 102, 0), then: After::Again },
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_purchase(
    rec: &Value,
    stats: &mut Stats,
    gear: &mut Gear,
    class: CharacterClass,
    lives: &mut Lives,
    life_price: &mut ExtraLifePrice,
    flags: &mut super::dialog::StoryFlags,
    data: &super::GameDataRef,
) {
    match rec["kind"].as_str() {
        Some("consumable") => match rec["counter"].as_str() {
            Some("herbs") => stats.herbs += 1,
            Some("antidotes") => stats.antidotes += 1,
            Some("bells") => {
                stats.bells += 1;
                stats.max_hp = (stats.max_hp + 8).min(255);
            }
            _ => {}
        },
        Some("extra_life") => {
            lives.0 += 1;
            life_price.0 = (life_price.0 * 2).min(6400);
        }
        Some("key_item") => {
            flags.0.insert(0xFF0AD3, 1);
        }
        Some("equipment") => {
            let tier = rec["tier"].as_u64().unwrap_or(0) as usize;
            match rec["slot"].as_str() {
                Some("fighter_weapon") => gear.ac6 = tier,
                Some("fighter_armor") => gear.ac0 = tier,
                Some("fighter_shield") => gear.ac4 = tier,
                Some("fighter_helm") => gear.ac2 = tier,
                Some("mage_wand") => gear.aca = tier,
                Some("mage_robe") => gear.ac8 = tier,
                _ => {}
            }
            if let Some(d) = data.get() {
                super::player::recompute_stats(stats, gear, class, d);
            }
        }
        _ => {}
    }
}

/// Icons, prices and the cursor of the goods list (sprites over the list window).
#[allow(clippy::too_many_arguments)]
fn draw_list_sprites(
    commands: &mut Commands,
    server: &AssetServer,
    shop: &Shop,
    shop_def: &Value,
    windows: &Windows,
    roots: &Query<Entity, With<ScreenRoot>>,
    sprites: &Query<Entity, With<ShopSprite>>,
    life_price: &ExtraLifePrice,
) {
    for e in sprites {
        commands.entity(e).despawn();
    }
    let Ok(root) = roots.single() else { return };
    let cursors: Handle<Image> = server.load("ui/shop_cursors.png");
    // Cursor colour of the customer: P1 orange (tile 0xC24D), P2 blue (0xC24F).
    let cx0 = if shop.customer == 0 { 0.0 } else { 8.0 };
    let cursor_rect = Rect::new(cx0, 0.0, cx0 + 8.0, 8.0);
    // YES/NO cursor (window shape 5 at (224, 32), cursor cell (2,1) / (2,2)).
    if let Step::YesNo { yn, yes, .. } = &shop.step {
        if windows.get(*yn).is_some_and(|w| w.is_open()) {
            let y = 32 + if *yes { 8 } else { 16 };
            commands.spawn((
                Sprite { image: cursors.clone(), rect: Some(cursor_rect), ..default() },
                Anchor::TOP_LEFT,
                Transform::from_xyz((224 + 16) as f32, -(y as f32), z::HUD + 1.0),
                ChildOf(root),
                ShopSprite,
            ));
        }
    }
    // "WHO IS THIS FOR?" cursor (window shape 6 at (56,112), cell (8, 4 + choice)).
    if let Step::Who { win, choice } = &shop.step {
        if windows.get(*win).is_some_and(|w| w.is_open() && w.run_phase() == Some(RunPhase::Halted)) {
            let y = 112 + (4 + *choice as i32) * 8;
            commands.spawn((
                Sprite { image: cursors.clone(), rect: Some(cursor_rect), ..default() },
                Anchor::TOP_LEFT,
                Transform::from_xyz((56 + 8 * 8) as f32, -(y as f32), z::HUD + 1.0),
                ChildOf(root),
                ShopSprite,
            ));
        }
    }
    let Some(list) = shop.list else { return };
    let Some(w) = windows.get(list) else { return };
    if !w.is_open() {
        return;
    }
    let (wx, wy) = (48, 40);
    let icons: Handle<Image> = server.load("ui/shop_icons.png");
    let mut spawn = |image: Handle<Image>, rect: Rect, x: i32, y: i32| {
        commands.spawn((
            Sprite { image, rect: Some(rect), ..default() },
            Anchor::TOP_LEFT,
            Transform::from_xyz(x as f32, -(y as f32), z::HUD + 1.0),
            ChildOf(root),
            ShopSprite,
        ));
    };
    for item in shop_def["items"].as_array().into_iter().flatten() {
        let [ix, iy] = cell(&item["icon_cell"]);
        let icon = item["record"].as_u64().unwrap_or(0) as f32;
        spawn(icons.clone(), Rect::new(icon * 16.0, 0.0, icon * 16.0 + 16.0, 16.0), wx + ix * 8, wy + iy * 8);
        let [px, py] = cell(&item["price_cells"][0]);
        let price = item["price"].as_u64().map(|p| p as u32).unwrap_or(life_price.0);
        let digits = format!("{price:>4}");
        for (k, ch) in digits.chars().enumerate() {
            if let Some(d) = ch.to_digit(10) {
                // Digits come from the font image (glyph 0x30 + d).
                let font: Handle<Image> = server.load("ui/font.png");
                spawn(font, Rect::new(d as f32 * 8.0, 0.0, d as f32 * 8.0 + 8.0, 8.0), wx + (px + k as i32) * 8, wy + py * 8);
            }
        }
    }
    // Cursor next to the selected slot.
    if let Some(item) = item_in_slot(shop_def, shop.slot) {
        let [cx, cy] = cell(&item["cursor_cell"]);
        spawn(cursors, cursor_rect, wx + cx * 8, wy + cy * 8);
    }
}

fn cell(v: &Value) -> [i32; 2] {
    [v[0].as_i64().unwrap_or(0) as i32, v[1].as_i64().unwrap_or(0) as i32]
}

pub fn shop_closed(shop: Option<Res<Shop>>) -> bool {
    shop.is_none()
}

pub fn plugin(app: &mut App) {
    app.init_resource::<ShopRequest>()
        .init_resource::<ExtraLifePrice>()
        .add_systems(
            FixedUpdate,
            (start_shop, run_shop)
                .chain()
                .in_set(GameSystems::Game)
                .before(super::player::tick)
                .run_if(in_state(AppState::InGame)),
        );
}
