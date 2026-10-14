//! Player ↔ object interaction (docs/re/objects.md §4): melee hits on odd ticks, contact damage,
//! pickups, NPC talk and solid objects on even ticks.
use super::{
    super::{
        anim::SpriteAnimator,
        coords::{Velocity, WorldPos, px},
        GameDataRef as GameDataHandleRef,
        data::SpriteSheet,
        dialog::{Dialog, StoryFlags},
        input::Joypads,
        player::{self, Action, CharacterClass, Facing, Player, PlayerState, Stats},
    },
    Obj, flag,
};
use crate::{GameClock, controller::Button};
use bevy::prelude::*;
use super::super::audio::{MUSIC_TIMER, PlaySound, SetMusicTimer, level_up_jingle};

/// Axis-aligned box: centre x, centre y, full width and height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitBox {
    pub cx: i32,
    pub cy: i32,
    pub w: i32,
    pub h: i32,
}

impl HitBox {
    /// Box (w, h, dx, dy) of an actor at (x, y): centred on x+dx, bottom at y+dy.
    pub fn from_header(x: i32, y: i32, w: i16, h: i16, dx: i16, dy: i16) -> Option<Self> {
        if w <= 0 || h <= 0 || w >= 30000 || dx >= 30000 {
            return None;
        }
        let (w, h) = (w as i32, h as i32);
        Some(Self { cx: x + dx as i32, cy: y + dy as i32 - h / 2, w, h })
    }
    /// Cartridge overlap test (0x3B00/0x3B34): inclusive, half-sizes truncated.
    pub fn overlaps(&self, o: &HitBox) -> bool {
        (self.cx - o.cx).abs() <= self.w / 2 + o.w / 2 && (self.cy - o.cy).abs() <= self.h / 2 + o.h / 2
    }
}

/// Hurt and attack boxes of an animated actor's current frame.
pub fn frame_boxes(sheet: &SpriteSheet, a: &SpriteAnimator, pos: &WorldPos) -> (Option<HitBox>, Option<HitBox>) {
    let Some(def) = sheet.anims.get(&a.anim) else { return (None, None) };
    let Some(af) = def.frames.get(a.index) else { return (None, None) };
    let b = sheet.frames[af.frame].boxes;
    let (x, y) = (pos.px_x(), pos.px_y());
    (
        HitBox::from_header(x, y, b[0], b[1], b[2], b[3]),
        HitBox::from_header(x, y, b[4], b[5], b[6], b[7]),
    )
}

/// Boxes of a player's frame as it will be after this tick's animation step. The cartridge steps
/// the player's animation inside the player update (0xC94C), before contact (0x9FF8), the object
/// update's melee test (0x14366) and the object scripts look at it; the remake steps all
/// animations later, in [`GameSystems::Animate`](crate::GameSystems).
pub fn player_boxes(sheet: &SpriteSheet, a: &SpriteAnimator, pos: &WorldPos) -> (Option<HitBox>, Option<HitBox>) {
    if a.manual {
        return frame_boxes(sheet, a, pos);
    }
    let mut next = a.clone();
    super::super::anim::step(sheet, &mut next);
    frame_boxes(sheet, &next, pos)
}

/// Player melee against objects (cartridge 0x14366, odd ticks).
#[allow(clippy::type_complexity)]
pub fn player_hits_objects(
    clock: Res<GameClock>,
    sheets: Res<Assets<SpriteSheet>>,
    data: GameDataHandleRef,
    mut hud: ResMut<super::super::hud_message::HudMessages>,
    mut sound: MessageWriter<PlaySound>,
    mut timer: MessageWriter<SetMusicTimer>,
    mut players: Query<(&mut Player, &CharacterClass, &mut Stats, &WorldPos, &SpriteAnimator), Without<Obj>>,
    mut objs: Query<(&mut Obj, &WorldPos, &SpriteAnimator)>,
) {
    let odd = clock.tick % 2 == 1;
    for (pl, class, mut stats, ppos, panim) in players.iter_mut() {
        let pi = pl.slot.min(1);
        let power = pl.attack_power();
        if power == 0 {
            for (mut o, ..) in &mut objs {
                o.hit_by[pi] = false;
            }
            continue;
        }
        if !odd {
            continue;
        }
        let Some(psheet) = sheets.get(&panim.sheet) else { continue };
        let Some(atk) = player_boxes(psheet, panim, ppos).1 else { continue };
        let mut boss_damage = 0;
        for (mut o, opos, oanim) in &mut objs {
            if o.delete || o.flags & flag::UNHITTABLE != 0 || o.hit_by[pi] {
                continue;
            }
            let Some(osheet) = sheets.get(&oanim.sheet) else { continue };
            let Some(hurt) = super::ops::object_boxes(&o, osheet, oanim, opos).0 else { continue };
            if !atk.overlaps(&hurt) {
                continue;
            }
            o.hit_by[pi] = true;
            // Clang (B9) / no effect (A5), hit (A7), kill (A8): cartridge 0x143A6.
            if o.flags & flag::CLANG != 0 {
                sound.write(PlaySound(0xB9));
                continue;
            }
            if o.flags & flag::MELEE_PROOF != 0 {
                sound.write(PlaySound(0xA5));
                continue;
            }
            let dmg = (power - o.defence as i32 / 2).max(1);
            if o.flags & flag::BOSS_PART != 0 {
                boss_damage += dmg;
                sound.write(PlaySound(0xA7));
                continue;
            }
            o.hp -= dmg;
            if o.hp <= 0 {
                sound.write(PlaySound(0xA8));
                kill(&mut o, &mut stats, *class, pl.slot, &data, &mut hud, &mut sound, &mut timer);
            } else {
                sound.write(PlaySound(0xA7));
                o.hit_count += 1;
                o.flash = 1;
            }
        }
        if boss_damage > 0 {
            for (mut o, ..) in &mut objs {
                if o.slot == 0 && !o.delete {
                    o.hp -= boss_damage;
                    o.hit_count += 1;
                    if o.hp <= 0 {
                        kill(&mut o, &mut stats, *class, pl.slot, &data, &mut hud, &mut sound, &mut timer);
                    } else {
                        o.flash = 1;
                    }
                }
            }
        }
        let _ = pl;
    }
}

/// The killer gets the EXP (cartridge 0x147CA: the hitting player, object +106 owner).
fn kill(
    o: &mut Obj,
    stats: &mut Stats,
    class: CharacterClass,
    slot: usize,
    data: &GameDataHandleRef,
    hud: &mut super::super::hud_message::HudMessages,
    sound: &mut MessageWriter<PlaySound>,
    timer: &mut MessageWriter<SetMusicTimer>,
) {
    o.hp = 0;
    o.pending_death = true;
    o.hit_kill = true;
    o.flags |= flag::UNHITTABLE | flag::HARMLESS;
    stats.exp = stats.exp.saturating_add(o.exp as u32).min(65535);
    if let Some(d) = data.get() {
        let spells_before = stats.spells_known;
        if player::check_level_up(stats, class, d) {
            hud.push_to(slot, super::super::lifecycle::msg::LEVEL_UP);
            level_up_jingle(sound, timer);
            // A newly memorized spell is announced (HUD messages 6..10).
            if class == CharacterClass::Mage && stats.spells_known > spells_before {
                hud.push_to(slot, 5 + stats.spells_known);
            }
        }
    }
}

/// Item types 3..=12 and the flag byte each one sets (cartridge 0xA220..).
fn item_flag(ty: u16) -> Option<u32> {
    Some(match ty {
        6 => 0xFF0ACC,
        7 => 0xFF0ACE,
        8 => 0xFF0AD0,
        9 => 0xFF0AD4,
        10 => 0xFF0AD6,
        11 => 0xFF0AD8,
        12 => 0xFF0ADA,
        _ => return None,
    })
}

/// Objects touching players: damage, pickups, NPC talk, solid push-out (cartridge 0x9FF8, even ticks).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn objects_touch_players(
    clock: Res<GameClock>,
    pads: Res<Joypads>,
    sheets: Res<Assets<SpriteSheet>>,
    mut dialog: ResMut<Dialog>,
    mut story: ResMut<StoryFlags>,
    mut persist: ResMut<super::PersistFlags>,
    mut hud: ResMut<super::super::hud_message::HudMessages>,
    room: Option<Res<super::super::room::CurrentRoom>>,
    mut shop_req: ResMut<super::super::shop::ShopRequest>,
    mut fx: ResMut<super::ops::Effects>,
    mut sound: MessageWriter<PlaySound>,
    mut players: Query<
        (&mut Player, &CharacterClass, &super::super::player_look::Gear, &mut Stats, &mut WorldPos, &mut Velocity, &SpriteAnimator),
        Without<Obj>,
    >,
    mut objs: Query<(&mut Obj, &WorldPos, &SpriteAnimator)>,
) {
    let even = clock.tick % 2 == 0;
    let room_index = room.map_or(0, |r| r.index);
    for (mut pl, class, gear, mut stats, mut ppos, mut pvel, panim) in &mut players {
        if pl.state >= PlayerState::Dying {
            continue;
        }
        let pad = pads.get(pl.slot);
        let Some(psheet) = sheets.get(&panim.sheet) else { continue };
        let Some(phurt) = player_boxes(psheet, panim, &ppos).0 else { continue };
        for (mut o, opos, oanim) in &mut objs {
            if o.delete {
                continue;
            }
            let Some(osheet) = sheets.get(&oanim.sheet) else { continue };
            let (ohurt, oatk) = super::ops::object_boxes(&o, osheet, oanim, opos);
            let body = ohurt.is_some_and(|h| h.overlaps(&phurt));
            // Pickups.
            if o.flags & flag::PICKUP != 0 && body {
                o.delete = true;
                if o.flags & flag::PERSIST_ON_PICKUP != 0 {
                    persist.0[(o.param118 & 0xFF) as usize] = 0xFF;
                }
                // Gold B8; items play the item jingle (music 0x90) and the room music comes back
                // when the music timer runs out (0xA2CC: [FF0A56] = 0x127).
                if o.ty == 2 {
                    sound.write(PlaySound(0xB8));
                } else {
                    sound.write(PlaySound(0x90));
                    fx.set_global(MUSIC_TIMER, 0x127);
                }
                match o.ty {
                    2 => stats.gold = (stats.gold + o.gold as u32).min(999_999),
                    3 => stats.herbs = (stats.herbs + 1).min(4),
                    4 => stats.elixirs = (stats.elixirs + 1).min(4),
                    5 => stats.antidotes = (stats.antidotes + 1).min(4),
                    t => {
                        if let Some(addr) = item_flag(t) {
                            story.0.insert(addr, 1);
                        }
                    }
                }
                if (3..=12).contains(&o.ty) {
                    // "{NAME} HAS OBTAINED ..." names the finder (0xFFE606 = player id).
                    dialog.open_for(0x48 + (o.ty as u32 - 3), 1, pl.slot);
                }
                continue;
            }
            // Talking to NPCs.
            if o.flags & flag::NPC != 0 && body && pad.pressed(Button::B) && !dialog.is_open() && shop_req.0.is_none() {
                if o.hp == 0 {
                    dialog.open_for(o.exp as u32, 0, pl.slot);
                } else {
                    // The talker is the customer.
                    shop_req.0 = Some(pl.slot);
                }
                continue;
            }
            // Solid objects push the player out horizontally by the overlap depth (0xA37A), which
            // leaves the boxes touching: still an overlap for touch conditions (locked doors, C70).
            if o.flags & flag::SOLID != 0 {
                if let Some(h) = ohurt {
                    if h.overlaps(&phurt) {
                        let half = h.w / 2 + phurt.w / 2;
                        let x = if ppos.px_x() < h.cx { h.cx - half } else { h.cx + half } - (phurt.cx - ppos.px_x());
                        ppos.x = px(x);
                    }
                }
            }
            if !even || o.flags & flag::HARMLESS != 0 || pl.invulnerable > 0 {
                continue;
            }
            // Shield block (0xA0E2): the Fighter with a shield, not attacking, facing a blockable
            // object that touches him destroys it (sound A5) instead of taking damage.
            if body
                && o.flags & flag::BLOCKABLE != 0
                && *class == CharacterClass::Fighter
                && gear.ac4 != 0
                && pl.action == Action::None
                && ohurt.is_some_and(|h| (h.cx >= phurt.cx) == (pl.facing == Facing::Right))
            {
                o.delete = true;
                sound.write(PlaySound(0xA5));
                continue;
            }
            let mut dmg = 0;
            if body {
                dmg = o.touch_dmg as i32;
            }
            if oatk.is_some_and(|a| a.overlaps(&phurt)) {
                dmg = dmg.max(o.attack_dmg as i32);
            }
            if dmg > 0 {
                let taken = if dmg < stats.def { 1 } else { (dmg - stats.def + 1).clamp(1, 20) };
                stats.hp -= taken;
                // Poisonous enemies (rooms 6+) poison half of the time: frame counter low byte < 0x80.
                if o.flags & flag::POISON != 0 && room_index >= 6 && (clock.tick & 0xFF) < 0x80 && !pl.poisoned {
                    pl.poisoned = true;
                    pl.poison_timer = 120;
                    hud.push_to(pl.slot, super::super::lifecycle::msg::POISONED);
                }
                player::knockback(&mut pl, &mut pvel);
                // Hurt A1, death A4 (0xA1CE / 0xA212).
                let died = super::super::lifecycle::after_damage(&mut stats, &mut pl, &mut hud);
                sound.write(PlaySound(if died { 0xA4 } else { 0xA1 }));
            }
        }
    }
}
