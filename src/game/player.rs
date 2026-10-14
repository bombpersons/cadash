//! The player character, following the cartridge's state machine (docs/re/physics.md §2):
//! input → state logic → gravity → collision → rope/hazard checks → animation.
use super::{
    anim::{SpriteAnimator, animated_sprite},
    collision::{self, Collider, Contacts},
    coords::{Velocity, WorldPos, px, z},
    data::{GameData, LevelStats, RoomDef, SpriteSheet},
    input::{Joypad, Joypads},
    room::CurrentRoom,
};
use crate::{GameSystems, controller::Button};
use bevy::prelude::*;
use super::audio::PlaySound;

/// Physics constants (16.16 fixed point px per tick).
pub mod consts {
    pub const GRAVITY: i32 = 0x3000;
    pub const MAX_FALL: i32 = 0x4_0000;
    pub const JUMP_VELOCITY: i32 = -0x4_4000;
    /// Falling faster than this while walking/standing means the ground disappeared.
    pub const LEDGE_FALL_SPEED: i32 = 0xC000;
    pub const ROPE_JUMP_VX: i32 = 0x1_8000;
    pub const ROPE_JUMP_VY: i32 = -0x3_8000;
    pub const KNOCKBACK_VX: i32 = 0x2_0000;
    pub const KNOCKBACK_VY: i32 = -0x2_0000;
    pub const HURT_IFRAMES: u32 = 91;
    pub const HAZARD_DAMAGE: i32 = 2;
    pub const STAND_HEIGHT: i32 = 64;
    pub const DOWN_THRUST_HEIGHT: i32 = 48;
    pub const HALF_WIDTH: i32 = 8;
    pub const CLIMB_HALF_WIDTH: i32 = 24;
}
use consts::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Facing {
    Left,
    #[default]
    Right,
}

impl Facing {
    pub fn suffix(self) -> &'static str {
        match self {
            Facing::Right => "_r",
            Facing::Left => "_l",
        }
    }
    pub fn sign(self) -> i32 {
        match self {
            Facing::Right => 1,
            Facing::Left => -1,
        }
    }
}

/// Cartridge player states (+122). Ordering matches the cartridge's numeric comparisons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum PlayerState {
    #[default]
    Stand,
    Walk,
    Crouch,
    Jump,
    /// Falling after leaving a rope.
    Fall,
    ClimbIdle,
    ClimbMove,
    Hurt,
    Dying,
    Dead,
}

impl PlayerState {
    fn climbing(self) -> bool {
        matches!(self, PlayerState::ClimbIdle | PlayerState::ClimbMove)
    }
}

/// Attack sub-state (+120).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Action {
    #[default]
    None,
    UpAttack,
    Attack,
    DownThrust,
}

/// Number of players in the current game (cartridge 0xFF0A40 + 1). Player slot `n` (0-based)
/// is controlled by pad `n + 1` and corresponds to the cartridge struct 0xFF0DC0 + n * 0x180.
#[derive(Resource, Debug, Clone, Copy)]
pub struct Party {
    pub count: usize,
}

impl Default for Party {
    fn default() -> Self {
        Self { count: 1 }
    }
}

impl Party {
    pub fn two_players(&self) -> bool {
        self.count > 1
    }
}

#[derive(Component, Debug, Default)]
pub struct Player {
    /// Player slot: 0 = player 1, 1 = player 2 (selects the pad, HUD line, name, windows).
    pub slot: usize,
    /// x at the start of the tick (cartridge +134), restored when the screen clamp hits the top.
    pub prev_x: i32,
    pub facing: Facing,
    pub state: PlayerState,
    pub action: Action,
    /// Holding onto a rope (+100); re-grabbing waits until the rope probe leaves the rope.
    pub rope_latch: bool,
    /// Invulnerability ticks remaining (+132).
    pub invulnerable: u32,
    /// Ticks the player is frozen (stair warps, transitions).
    pub freeze: u32,
    /// Attack power of the current swing (cartridge +124); 0 when not attacking.
    pub power: i32,
    /// Increments for every new swing (objects take one hit per swing).
    pub swing_id: u32,
    /// Shrunken walk-only mode (+116 = 1, room 12 attributes 0x60/0x61).
    pub small: bool,
    /// Poisoned (+0xA6) and ticks until the next poison damage (+0xA8).
    pub poisoned: bool,
    pub poison_timer: u32,
    /// Ticks after walking off a ledge during which moving platforms don't catch the player (+126).
    pub ledge_timer: u32,
    /// Walk speed forced by a room script (16.16; cartridge +0xA2 = 1.0 in the ending room 47),
    /// instead of the class/stat speed.
    pub walk_speed: Option<i32>,
}

impl Player {
    pub fn attack_power(&self) -> i32 {
        self.power
    }
}

/// Which character class the player controls.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharacterClass {
    Fighter,
    Mage,
}

impl CharacterClass {
    pub fn sheet_path(self) -> &'static str {
        match self {
            CharacterClass::Fighter => "sprites/player/fighter.sheet.json",
            CharacterClass::Mage => "sprites/player/mage.sheet.json",
        }
    }
    /// Base walk speed before the speed stat is added.
    pub fn base_speed(self) -> i32 {
        match self {
            CharacterClass::Fighter => 0x1_8000,
            CharacterClass::Mage => 0x1_6000,
        }
    }
}

/// RPG statistics of the player.
#[derive(Component, Debug, Clone)]
pub struct Stats {
    pub level: usize,
    pub exp: u32,
    pub hp: i32,
    pub max_hp: i32,
    pub mp: i32,
    pub max_mp: i32,
    pub atk: i32,
    pub def: i32,
    pub speed: i32,
    pub attack_frame: u16,
    pub gold: u32,
    pub herbs: u32,
    pub elixirs: u32,
    pub antidotes: u32,
    /// Bells of Life bought (+8 max HP each).
    pub bells: u32,
    /// Highest spell index the Mage knows (player +0x9C).
    pub spells_known: u32,
    /// Strength / armour class from equipment (tables 0xC790..).
    pub bonus_atk: i32,
    pub bonus_def: i32,
}

/// Per-slot equipment bonuses by tier (cartridge tables 0xC790..0xC7A6): (STR, A.C.).
/// The equipment slots are global; each class only uses its own (0xC5B0: Fighter weapon/armor/
/// helm/shield, Mage wand/robe).
pub fn gear_bonus(g: &super::player_look::Gear, class: CharacterClass) -> (i32, i32) {
    const WEAPON: [i32; 5] = [0, 6, 5, 16, 26];
    const ARMOR: [i32; 4] = [0, 3, 14, 22];
    const HELM: [i32; 3] = [0, 2, 2];
    const SHIELD: [i32; 4] = [0, 3, 14, 20];
    const WAND: [i32; 4] = [0, 6, 9, 19];
    const ROBE: [i32; 3] = [0, 3, 13];
    let at = |t: &[i32], i: usize| t.get(i).copied().unwrap_or(0);
    match class {
        CharacterClass::Fighter => (at(&WEAPON, g.ac6), at(&ARMOR, g.ac0) + at(&HELM, g.ac2) + at(&SHIELD, g.ac4)),
        CharacterClass::Mage => (at(&WAND, g.aca), at(&ROBE, g.ac8)),
    }
}

/// Recomputes strength/armour from the level table and equipment (cartridge 0xC5B0);
/// HP/MP maxima and current values are untouched.
pub fn recompute_stats(stats: &mut Stats, gear: &super::player_look::Gear, class: CharacterClass, data: &GameData) {
    let levels = match class {
        CharacterClass::Fighter => &data.levels.fighter,
        CharacterClass::Mage => &data.levels.mage,
    };
    let (ba, bd) = gear_bonus(gear, class);
    stats.bonus_atk = ba;
    stats.bonus_def = bd;
    if let Some(l) = levels.get(stats.level) {
        stats.atk = l.atk as i32 + ba;
        stats.def = l.def as i32 + bd;
    }
}

/// Raises the level while experience reaches the next threshold (cartridge 0xC542).
/// Maximum HP/MP and stats grow; current HP/MP are not refilled.
pub fn check_level_up(stats: &mut Stats, class: CharacterClass, data: &GameData) -> bool {
    let levels = match class {
        CharacterClass::Fighter => &data.levels.fighter,
        CharacterClass::Mage => &data.levels.mage,
    };
    let mut up = false;
    while let Some(next) = levels.get(stats.level + 1) {
        if stats.exp < next.exp {
            break;
        }
        stats.level += 1;
        stats.max_hp = (next.hp as i32 + 8 * stats.bells as i32).min(255);
        stats.max_mp = next.mp as i32;
        stats.atk = next.atk as i32 + stats.bonus_atk;
        stats.def = next.def as i32 + stats.bonus_def;
        stats.speed = next.speed as i32;
        stats.attack_frame = next.attack_frame;
        stats.spells_known = next.spells;
        up = true;
    }
    up
}

impl Stats {
    pub fn from_level(level: usize, s: &LevelStats) -> Self {
        Self {
            level,
            exp: s.exp,
            hp: s.hp as i32,
            max_hp: s.hp as i32,
            mp: s.mp as i32,
            max_mp: s.mp as i32,
            atk: s.atk as i32,
            def: s.def as i32,
            speed: s.speed as i32,
            attack_frame: s.attack_frame,
            gold: 0,
            herbs: 0,
            elixirs: 0,
            antidotes: 0,
            bells: 0,
            spells_known: s.spells,
            bonus_atk: 0,
            bonus_def: 0,
        }
    }
}

pub fn spawn_player(
    commands: &mut Commands,
    slot: usize,
    sheet: Handle<SpriteSheet>,
    class: CharacterClass,
    stats: Stats,
    pos: WorldPos,
    facing: Facing,
) -> Entity {
    let mut animator = SpriteAnimator::new(sheet, format!("stand{}", facing.suffix()), z::PLAYER - slot as f32 * 0.01);
    animator.actor_duration = stats.attack_frame;
    commands
        .spawn((
            Name::new(format!("player {}", slot + 1)),
            Player { slot, facing, ..default() },
            class,
            stats,
            pos,
            Velocity::default(),
            Collider::default(),
            Contacts::default(),
            super::player_look::Gear::default(),
            super::lifecycle::DeathState::default(),
            animated_sprite(animator),
        ))
        .id()
}

fn start_attack(p: &mut Player, vel: &mut Velocity, pad: &Joypad) {
    p.action = if pad.held(Button::Up) { Action::UpAttack } else { Action::Attack };
    p.state = PlayerState::Stand;
    vel.x = 0;
    vel.y = 0;
}

fn jump_pressed(pad: &Joypad) -> bool {
    pad.pressed(Button::A) || pad.pressed(Button::C)
}

fn horizontal_input(pad: &Joypad) -> Option<Facing> {
    match (pad.held(Button::Left), pad.held(Button::Right)) {
        (true, false) => Some(Facing::Left),
        (false, true) => Some(Facing::Right),
        _ => None,
    }
}

/// State logic for one tick (cartridge handlers 0x97EA.. / attack handler 0x9C02).
fn think(p: &mut Player, vel: &mut Velocity, c: &Contacts, pad: &Joypad, walk: i32, anim_done: bool) {
    let dir = horizontal_input(pad);
    // Walk-only mode (cartridge 0x9F80): 1 px/tick, no jumping, attacking or climbing.
    if p.small && p.state < PlayerState::Hurt {
        p.action = Action::None;
        match dir {
            Some(d) => {
                p.facing = d;
                p.state = PlayerState::Walk;
                vel.x = d.sign() * 0x1_0000;
            }
            None => {
                p.state = PlayerState::Stand;
                vel.x = 0;
            }
        }
        return;
    }
    if p.action != Action::None {
        attack_logic(p, vel, c, pad, walk, anim_done);
        return;
    }
    match p.state {
        PlayerState::Stand | PlayerState::Walk => {
            if vel.y > 0 && vel.y >= LEDGE_FALL_SPEED {
                p.state = PlayerState::Jump;
                vel.x = 0;
                p.ledge_timer = 8;
            } else if pad.pressed(Button::B) {
                start_attack(p, vel, pad);
            } else if pad.held(Button::Down) {
                p.state = PlayerState::Crouch;
                vel.x = 0;
                vel.y = 0;
            } else if jump_pressed(pad) {
                p.state = PlayerState::Jump;
                vel.y = JUMP_VELOCITY;
                vel.x = dir.map_or(0, |d| d.sign() * walk);
                if let Some(d) = dir {
                    p.facing = d;
                }
            } else if let Some(d) = dir {
                p.facing = d;
                p.state = PlayerState::Walk;
                vel.x = d.sign() * walk;
            } else {
                p.state = PlayerState::Stand;
                vel.x = 0;
            }
        }
        PlayerState::Crouch => {
            if let Some(d) = dir {
                p.facing = d;
            }
            if !pad.held(Button::Down) {
                p.state = PlayerState::Stand;
            } else if pad.pressed(Button::B) {
                p.action = Action::Attack;
            }
            vel.x = 0;
        }
        PlayerState::Jump | PlayerState::Fall => {
            // 0x99D2 (state handler only, not during attacks).
            p.ledge_timer = p.ledge_timer.saturating_sub(1);
            if let Some(d) = dir {
                p.facing = d;
            }
            if c.landed() {
                p.state = PlayerState::Stand;
                vel.x = 0;
                vel.y = 0;
            } else if pad.pressed(Button::B) {
                p.action = if pad.held(Button::Up) {
                    Action::UpAttack
                } else if pad.held(Button::Down) {
                    Action::DownThrust
                } else {
                    Action::Attack
                };
            }
        }
        PlayerState::ClimbIdle | PlayerState::ClimbMove => {
            if let Some(d) = dir {
                p.facing = d;
            }
            if !p.rope_latch {
                p.state = PlayerState::Fall;
            } else if c.landed() {
                p.state = PlayerState::Stand;
                p.rope_latch = false;
            } else if pad.pressed(Button::B) {
                p.action = Action::Attack;
                vel.y = 0;
            } else if jump_pressed(pad) {
                vel.x = p.facing.sign() * ROPE_JUMP_VX;
                vel.y = ROPE_JUMP_VY;
                p.state = PlayerState::Jump;
            } else {
                vel.x = 0;
                vel.y = 0;
                if pad.held(Button::Up) {
                    p.state = PlayerState::ClimbMove;
                    vel.y = -walk;
                } else if pad.held(Button::Down) {
                    p.state = PlayerState::ClimbMove;
                    vel.y = walk;
                } else {
                    p.state = PlayerState::ClimbIdle;
                }
            }
        }
        PlayerState::Hurt => {
            if vel.y >= 0 && c.landed() {
                p.state = PlayerState::Stand;
                vel.x = 0;
                vel.y = 0;
            }
        }
        PlayerState::Dying | PlayerState::Dead => {
            // States 8/9 (0x9DCA / 0x9E2A) only animate: the body stays where it fell.
            vel.x = 0;
            vel.y = 0;
        }
    }
}

fn attack_logic(p: &mut Player, vel: &mut Velocity, c: &Contacts, pad: &Joypad, walk: i32, anim_done: bool) {
    let dir = horizontal_input(pad);
    match p.state {
        PlayerState::Stand | PlayerState::Walk | PlayerState::Crouch => {
            if anim_done {
                p.action = Action::None;
                if p.state != PlayerState::Crouch {
                    p.state = PlayerState::Stand;
                }
                return;
            }
            if let Some(d) = dir {
                p.facing = d;
            }
            if p.action == Action::Attack {
                if pad.held(Button::Down) {
                    p.state = PlayerState::Crouch;
                } else if p.state == PlayerState::Crouch {
                    p.state = PlayerState::Stand;
                }
            }
            vel.x = 0;
            if jump_pressed(pad) && p.state != PlayerState::Crouch {
                p.state = PlayerState::Jump;
                vel.y = JUMP_VELOCITY;
                vel.x = dir.map_or(0, |d| d.sign() * walk);
            }
        }
        PlayerState::Jump | PlayerState::Fall => {
            if let Some(d) = dir {
                p.facing = d;
            }
            if c.landed() {
                vel.x = 0;
                if p.action == Action::DownThrust {
                    p.action = Action::None;
                    p.state = PlayerState::Crouch;
                } else if anim_done {
                    p.action = Action::None;
                    p.state = PlayerState::Stand;
                } else {
                    p.state = PlayerState::Stand;
                }
            }
        }
        PlayerState::ClimbIdle | PlayerState::ClimbMove => {
            vel.x = 0;
            vel.y = 0;
            if anim_done {
                p.action = Action::None;
            }
        }
        _ => p.action = Action::None,
    }
}

fn anim_base(p: &Player) -> &'static str {
    use PlayerState::*;
    if p.small && p.state < Dying {
        return if p.state == Walk { "small_walk" } else { "small" };
    }
    match (p.state, p.action) {
        (Hurt, _) => "hurt",
        (Dying | Dead, _) => "dying",
        (ClimbIdle | ClimbMove, Action::None) => {
            if p.state == ClimbIdle { "climb" } else { "climb_move" }
        }
        (ClimbIdle | ClimbMove, _) => "attack_climb",
        (Crouch, Action::None) => "crouch",
        (Crouch, Action::UpAttack) => "attack_up_crouch",
        (Crouch, _) => "attack_crouch",
        (Jump | Fall, Action::None) => "jump",
        (Jump | Fall, Action::UpAttack) => "attack_up_jump",
        (Jump | Fall, Action::Attack) => "attack_jump",
        (Jump | Fall, Action::DownThrust) => "down_thrust",
        (Stand | Walk, Action::UpAttack) => "attack_up",
        (Stand | Walk, Action::Attack | Action::DownThrust) => "attack",
        (Walk, Action::None) => "walk",
        (Stand, Action::None) => "stand",
    }
}

fn attack_family(name: &str) -> bool {
    name.starts_with("attack")
}

fn update_animation(p: &Player, anim: &mut SpriteAnimator) {
    let base = anim_base(p);
    let want = format!("{base}{}", p.facing.suffix());
    if anim.anim == want {
        return;
    }
    let current_base = anim.anim.trim_end_matches("_l").trim_end_matches("_r");
    // Facing changes and attack posture changes keep the current frame.
    if current_base == base || (attack_family(current_base) && attack_family(base) && !current_base.contains("up") && !base.contains("up")) {
        anim.swap_keep_frame(&want);
    } else {
        anim.restart(&want);
    }
}

/// Runs the whole player tick.
#[allow(clippy::type_complexity)]
pub fn tick(
    pads: Res<Joypads>,
    room: Option<Res<CurrentRoom>>,
    rooms: Res<Assets<RoomDef>>,
    data: Res<super::GameDataHandle>,
    datas: Res<Assets<GameData>>,
    mut hud: ResMut<super::hud_message::HudMessages>,
    story: Res<super::dialog::StoryFlags>,
    mut sound: MessageWriter<PlaySound>,
    mut q: Query<(
        &mut Player,
        &CharacterClass,
        &mut Stats,
        &mut WorldPos,
        &mut Velocity,
        &mut Collider,
        &mut Contacts,
        &mut SpriteAnimator,
    )>,
) {
    let Some(room) = room else { return };
    let (Some(def), Some(data)) = (rooms.get(&room.def), datas.get(&data.0)) else { return };
    for (mut p, class, mut stats, mut pos, mut vel, mut col, mut c, mut anim) in &mut q {
        let pad = pads.get(p.slot);
        p.prev_x = pos.x;
        if p.freeze > 0 {
            p.freeze -= 1;
            continue;
        }
        let walk = p.walk_speed.unwrap_or(class.base_speed() + stats.speed * 256);
        anim.actor_duration = stats.attack_frame;
        let anim_done = anim.cycled;
        let prev_state = p.state;
        let prev_action = p.action;
        think(&mut p, &mut vel, &c, &pad, walk, anim_done);
        if p.action != prev_action || p.state != prev_state {
            anim.cycled = false;
        }
        if prev_action == Action::None && p.action != Action::None {
            p.swing_id = p.swing_id.wrapping_add(1);
            p.power = stats.atk;
            // Swing sound per class (0xA71A, table 0xA732).
            sound.write(PlaySound(match class {
                CharacterClass::Fighter => 0x98,
                CharacterClass::Mage => 0x99,
            }));
        } else if p.action == Action::None {
            p.power = 0;
        }
        // Jump from standing or walking (0x98B6); not off a rope or out of an attack.
        if prev_action == Action::None
            && matches!(prev_state, PlayerState::Stand | PlayerState::Walk)
            && p.state == PlayerState::Jump
            && vel.y == JUMP_VELOCITY
        {
            sound.write(PlaySound(0xA0));
        }

        // Collision body for this tick.
        col.half_width = if p.state.climbing() { CLIMB_HALF_WIDTH } else { HALF_WIDTH };
        col.height = if p.small {
            32
        } else if p.action == Action::DownThrust {
            DOWN_THRUST_HEIGHT
        } else {
            STAND_HEIGHT
        };

        // Handler paths that return early skip gravity, physics, hazard and rope checks for the
        // tick (the footing flags stay as they were): landing from a jump/fall (0x99EE), and leaving
        // a rope by losing it, attacking or jumping off (0x9AA6 / 0x9ABE / 0x9B38).
        let landing = matches!(prev_state, PlayerState::Jump | PlayerState::Fall)
            && prev_action == Action::None
            && p.action == Action::None
            && p.state == PlayerState::Stand
            && c.landed();
        let left_rope = prev_state.climbing()
            && prev_action == Action::None
            && (p.action != Action::None || matches!(p.state, PlayerState::Jump | PlayerState::Fall));
        if !(landing || left_rope) {
            // Gravity applies except while climbing (and when dead).
            let gravity = !p.state.climbing() && !matches!(p.state, PlayerState::Dying | PlayerState::Dead);
            if gravity {
                vel.y += GRAVITY;
                if vel.y > MAX_FALL {
                    vel.y = MAX_FALL;
                }
            }
            collision::step(def, data, &col, &mut pos, &mut vel, &mut c);

            // Hazard tiles (attr 0x02) at the feet; the Scale of Mermaid (0xFF0ACC) protects except in
            // room 22 (0xA508).
            let immune = room.index != 22 && story.0.get(&0xFF0ACC).is_some_and(|&v| v != 0);
            if def.attr_at(pos.px_x(), pos.px_y() - 1) == 0x02 && p.invulnerable == 0 && p.state < PlayerState::Dying && !immune {
                stats.hp -= HAZARD_DAMAGE;
                knockback(&mut p, &mut vel);
                // Death A4; otherwise 9A, or the plain hurt sound A1 in room 22 (0xA5F8).
                let died = super::lifecycle::after_damage(&mut stats, &mut p, &mut hud);
                sound.write(PlaySound(if died { 0xA4 } else if room.index == 22 { 0xA1 } else { 0x9A }));
            }

            // Room 12's shrink/grow attributes 0x60/0x61 (cartridge 0xC45A) are handled by `room_events`.

            // Ropes / ladders (attr 0x01) probed at mid-body.
            let rope = !p.small && def.attr_at(pos.px_x(), pos.px_y() - col.height / 2) == 0x01;
            if rope {
                if !c.ground && !c.on_surface && !p.rope_latch && p.state < PlayerState::Hurt {
                    pos.x = px((pos.px_x() & !15) | 8);
                    vel.x = 0;
                    vel.y = 0;
                    p.rope_latch = true;
                    p.state = PlayerState::ClimbIdle;
                    p.action = Action::None;
                }
            } else {
                p.rope_latch = false;
            }
        }

        // Hurt while on a rope (+102 still set): back to climbing after the physics step (0x9DB6).
        if prev_state == PlayerState::Hurt && p.state == PlayerState::Hurt && p.rope_latch {
            p.state = PlayerState::ClimbIdle;
        }

        if p.invulnerable > 0 {
            p.invulnerable -= 1;
        }
        if p.state == PlayerState::Dying && anim.cycled {
            p.state = PlayerState::Dead;
        }
        update_animation(&p, &mut anim);
    }
}

/// Knock the player backwards (cartridge 0xA850).
pub fn knockback(p: &mut Player, vel: &mut Velocity) {
    p.invulnerable = HURT_IFRAMES;
    p.action = Action::None;
    vel.x = 0;
    vel.y = 0;
    // On a rope there is no knockback; the hurt state re-grabs the rope after one physics step.
    if !p.state.climbing() {
        vel.y = KNOCKBACK_VY;
        vel.x = -p.facing.sign() * KNOCKBACK_VX;
        p.rope_latch = false;
    }
    p.state = PlayerState::Hurt;
}

/// Blink while invulnerable (sprite hidden on odd timer values).
pub fn blink(mut q: Query<(&Player, &mut Visibility)>) {
    for (p, mut vis) in &mut q {
        // After the death burst the player is not drawn until revived.
        let hidden = p.invulnerable % 2 == 1 || p.state == PlayerState::Dead;
        *vis = if hidden { Visibility::Hidden } else { Visibility::Inherited };
    }
}

/// Keep the players on screen (cartridge 0xA736): x within screen 16..296; with two players also
/// feet within playfield y 68..188 (72..184 while every living player climbs), and x restored to the
/// previous tick's when pushed down from the top. Dying/dead players are not clamped.
///
/// The cartridge applies the vertical rule in one-player games too, but there the camera follows the
/// single player so it practically never engages; it is limited to two players here to keep the
/// one-player game unchanged.
pub fn clamp_to_screen(
    cam: Res<super::camera::CameraPos>,
    party: Res<Party>,
    room: Option<Res<CurrentRoom>>,
    mut q: Query<(&Player, &mut WorldPos)>,
) {
    let all_climbing = q.iter().filter(|(p, _)| p.state < PlayerState::Dying).all(|(p, _)| p.state.climbing());
    for (p, mut pos) in &mut q {
        if party.two_players() && (p.state >= PlayerState::Dying || room.as_ref().is_some_and(|r| r.index == 47)) {
            continue;
        }
        let x = pos.px_x().clamp(cam.x + 16, cam.x + 296);
        if x != pos.px_x() {
            pos.x = px(x);
        }
        if !party.two_players() {
            continue;
        }
        let (top, bottom) = if all_climbing { (72, 184) } else { (68, 188) };
        if pos.px_y() <= cam.y + top {
            pos.y = px(cam.y + top);
            pos.x = p.prev_x;
        } else if pos.px_y() > cam.y + bottom {
            pos.y = px(cam.y + bottom);
        }
    }
}

/// Poison: every 120 ticks an antidote is used if carried, otherwise 3 HP are lost (cartridge 0xA3EA).
pub fn poison(mut hud: ResMut<super::hud_message::HudMessages>, mut sound: MessageWriter<PlaySound>, mut q: Query<(&mut Player, &mut Stats)>) {
    for (mut p, mut s) in &mut q {
        if !p.poisoned || p.state >= PlayerState::Dying {
            continue;
        }
        p.poison_timer = p.poison_timer.saturating_sub(1);
        if p.poison_timer > 0 {
            continue;
        }
        p.poison_timer = 120;
        if s.antidotes > 0 {
            s.antidotes -= 1;
            p.poisoned = false;
            hud.push_to(p.slot, super::lifecycle::msg::ANTIDOTE);
        } else {
            s.hp -= 3;
            p.invulnerable = p.invulnerable.max(21);
            let died = super::lifecycle::after_damage(&mut s, &mut p, &mut hud);
            sound.write(PlaySound(if died { 0xA4 } else { 0xA1 }));
        }
    }
}

pub fn plugin(app: &mut App) {
    app.init_resource::<Party>().add_systems(
        FixedUpdate,
        (tick, poison, clamp_to_screen)
            .chain()
            .in_set(GameSystems::Game)
            .run_if(super::dialog::dialog_closed)
            .run_if(super::transition::no_transition)
            .run_if(super::room_events::world_running),
    )
        .add_systems(Update, blink);
}
