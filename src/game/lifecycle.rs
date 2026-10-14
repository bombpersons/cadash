//! Running out of HP, items that save you, death, CONTINUE? and GAME OVER
//! (docs/re/ui_rpg_data.md, docs/re/ui_screens.md "Continue / Game over").
use super::{
    AppState,
    hud_message::HudMessages,
    coords::{Velocity, WorldPos},
    input::Joypads,
    player::{Action, Player, PlayerState, Stats},
    transition::{FADE_TICKS, FadeTo, ScreenFade},
    ui::ScreenRoot,
};
use crate::{GameSystems, controller::Button};
use bevy::{prelude::*, sprite::Anchor};

/// HUD message ids (dialogue entry = 138 + id).
pub mod msg {
    pub const LEVEL_UP: u32 = 1;
    pub const DEAD: u32 = 2;
    pub const POISONED: u32 = 4;
    pub const HERB: u32 = 11;
    pub const ANTIDOTE: u32 = 12;
    pub const ELIXIR: u32 = 13;
    pub const GAME_OVER: u32 = 14;
    pub const PRESS_START: u32 = 15;
}

/// Remaining continues (cartridge 0xFF0A7A, 2 for one player / 4 for two (0x4592), max 9).
#[derive(Resource, Debug, Clone, Copy)]
pub struct Lives(pub u32);

impl Lives {
    /// Credits at the start of a game with `players` players.
    pub fn for_party(players: usize) -> Self {
        Self(2 * players.max(1) as u32)
    }
}

impl Default for Lives {
    fn default() -> Self {
        Self(2)
    }
}

/// Called whenever the player loses HP: herbs and elixirs are used automatically.
/// Returns true if the player died.
pub fn after_damage(stats: &mut Stats, p: &mut Player, hud: &mut HudMessages) -> bool {
    if stats.hp > 0 || p.state >= PlayerState::Dying {
        return false;
    }
    if stats.herbs > 0 {
        stats.herbs -= 1;
        stats.hp = 30.min(stats.max_hp);
        hud.push_to(p.slot, msg::HERB);
        false
    } else if stats.elixirs > 0 {
        stats.elixirs -= 1;
        stats.hp = stats.max_hp;
        stats.mp = stats.max_mp;
        p.poisoned = false;
        hud.push_to(p.slot, msg::ELIXIR);
        false
    } else {
        stats.hp = 0;
        p.state = PlayerState::Dying;
        p.action = Action::None;
        hud.push_to(p.slot, msg::DEAD);
        true
    }
}

#[derive(Component)]
struct ContinueSprite;

/// "CONTINUE? n" / "GAME OVER" overlay shown while every player is dead (cartridge 0x2BFA4).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndFlow {
    /// "CONTINUE? n" counting down (n from 10, one step per 60 ticks).
    Continue { n: u32, timer: u32 },
    /// "GAME OVER" typing (8 steps x 4 ticks) then 300 ticks and a fade.
    GameOver { timer: u32 },
}

/// Per-player bookkeeping of the dead state (cartridge state 9 handler 0x9E2A).
#[derive(Component, Debug, Default, Clone, Copy)]
pub struct DeathState {
    /// Taken out of the game while a partner lives on (cartridge: x = y = 0, +88/+102 = -1):
    /// revives at the partner's position.
    pub removed: bool,
    /// "GAME OVER" was announced on this player's status line (player +0xDE).
    pub no_credits_shown: bool,
}

/// Bookkeeping when a player's death animation ends (cartridge 0x9DCA): if a partner is still in the
/// game, the dead player leaves it (excluded from camera, exits and enemy targeting) and later
/// revives next to the partner.
fn mark_removed(mut players: Query<(&Player, &mut DeathState)>) {
    let not_dead = players.iter().filter(|(p, _)| p.state != PlayerState::Dead).count();
    for (p, mut d) in &mut players {
        if p.state == PlayerState::Dead && !d.removed && not_dead > 0 {
            d.removed = true;
        } else if p.state != PlayerState::Dead {
            d.removed = false;
        }
    }
}

/// Dead players: credits, HUD prompts and START to revive (cartridge 0x9E2A). Every dead player
/// revives on their own START press (one credit each), immediately in a two-player game while the
/// partner still plays, or during the CONTINUE? countdown when everybody is dead.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn revive(
    pads: Res<Joypads>,
    flow: Option<Res<EndFlow>>,
    mut commands: Commands,
    mut lives: ResMut<Lives>,
    mut hud: ResMut<HudMessages>,
    mut players: Query<(&mut Player, &mut Stats, &mut WorldPos, &mut Velocity, &mut DeathState)>,
) {
    if matches!(flow.as_deref(), Some(EndFlow::GameOver { .. })) {
        return;
    }
    let snapshot: Vec<(usize, WorldPos, bool)> = players.iter().map(|(p, _, pos, ..)| (p.slot, *pos, p.small)).collect();
    let mut revived = None;
    for (mut p, mut s, mut pos, mut vel, mut d) in &mut players {
        if p.state != PlayerState::Dead || revived.is_some() {
            continue;
        }
        if lives.0 == 0 {
            if !d.no_credits_shown {
                d.no_credits_shown = true;
                hud.push_to(p.slot, msg::GAME_OVER);
            }
            continue;
        }
        if d.no_credits_shown {
            // A credit was bought meanwhile.
            d.no_credits_shown = false;
            hud.push_to(p.slot, msg::PRESS_START);
        }
        if hud.busy(p.slot) || !pads.get(p.slot).pressed(Button::Start) {
            continue;
        }
        lives.0 -= 1;
        if d.removed {
            // Back in next to the partner (who is taken out in turn if also dead).
            // (x, y and the shrunken mode +116 are copied, 0x9EBE).
            if let Some(&(_, ppos, small)) = snapshot.iter().find(|(slot, ..)| *slot != p.slot) {
                *pos = ppos;
                p.small = small;
            }
        }
        d.removed = false;
        *vel = Velocity::default();
        s.hp = s.max_hp;
        s.mp = s.max_mp;
        p.state = PlayerState::Stand;
        p.action = Action::None;
        p.poisoned = false;
        p.rope_latch = false;
        p.invulnerable = 91;
        hud.dismiss(p.slot);
        revived = Some(p.slot);
    }
    if let Some(slot) = revived {
        for (p, _, _, _, mut d) in &mut players {
            if p.slot != slot && p.state == PlayerState::Dead {
                d.removed = true;
            }
        }
        commands.remove_resource::<EndFlow>();
    }
}

#[allow(clippy::too_many_arguments)]
fn run_end_flow(
    mut commands: Commands,
    server: Res<AssetServer>,
    flow: Option<ResMut<EndFlow>>,
    mut lives: ResMut<Lives>,
    mut fade: ResMut<ScreenFade>,
    roots: Query<Entity, With<ScreenRoot>>,
    sprites: Query<Entity, With<ContinueSprite>>,
    players: Query<&Player>,
    mut sound: MessageWriter<super::audio::PlaySound>,
    mut next: ResMut<NextState<AppState>>,
) {
    let Ok(root) = roots.single() else { return };
    // Redraw the overlay each tick.
    for e in &sprites {
        commands.entity(e).despawn();
    }
    let sprite = |path: String, x: i32, y: i32| {
        (
            Sprite::from_image(server.load(path)),
            Anchor::TOP_LEFT,
            Transform::from_xyz(x as f32, -(y as f32), super::coords::z::HUD + 3.0),
            ChildOf(root),
            ContinueSprite,
        )
    };
    let dead = players.iter().filter(|p| p.state == PlayerState::Dead).count();
    let all_dead = dead > 0 && dead == players.iter().count();
    // "CREDIT n" while anybody is dead (cartridge 0x2BEFE, mapping 0x2BF6A at screen (240,184)).
    if dead > 0 {
        commands.spawn(sprite("screens/continue_gameover/credit.png".into(), 240, 184));
        commands.spawn(sprite(format!("screens/continue_gameover/digit_{}.png", lives.0.min(9)), 296, 184));
    }
    let Some(mut flow) = flow else {
        if all_dead {
            let f = if lives.0 > 0 { EndFlow::Continue { n: 10, timer: 0 } } else { EndFlow::GameOver { timer: 0 } };
            commands.insert_resource(f);
        }
        return;
    };
    match *flow {
        EndFlow::Continue { n, timer } => {
            commands.spawn(sprite("screens/continue_gameover/continue.png".into(), 112, 88));
            commands.spawn(sprite(format!("screens/continue_gameover/digit_{}.png", (n - 1).min(9)), 192, 88));
            *flow = if timer + 1 >= 60 {
                if n <= 1 { EndFlow::GameOver { timer: 0 } } else { EndFlow::Continue { n: n - 1, timer: 0 } }
            } else {
                EndFlow::Continue { n, timer: timer + 1 }
            };
        }
        EndFlow::GameOver { timer } => {
            // "GAME OVER" appears with the game-over tune (0x2C09A).
            if timer == 0 {
                sound.write(super::audio::PlaySound(0x96));
            }
            let step = (timer / 4).min(7);
            commands.spawn(sprite(format!("screens/continue_gameover/gameover_{step}.png"), 120, 88));
            let t = timer + 1;
            let fade_start = 32 + 300;
            if t == fade_start {
                commands.insert_resource(FadeTo { target: 1.0, ticks: FADE_TICKS });
            }
            if t >= fade_start + FADE_TICKS && fade.0 >= 1.0 {
                commands.remove_resource::<EndFlow>();
                *lives = Lives::default();
                fade.0 = 1.0;
                next.set(AppState::Sega);
                return;
            }
            *flow = EndFlow::GameOver { timer: t };
        }
    }
}

/// The overlay is redrawn by `run_end_flow`, which stops running once the game is left: clear it.
fn clear_overlay(mut commands: Commands, sprites: Query<Entity, With<ContinueSprite>>) {
    for e in &sprites {
        commands.entity(e).despawn();
    }
}

fn reset_on_new_game(mut commands: Commands, mut lives: ResMut<Lives>) {
    *lives = Lives::default();
    commands.remove_resource::<EndFlow>();
}

pub fn plugin(app: &mut App) {
    app.init_resource::<Lives>()
        .add_systems(OnEnter(AppState::NameEntry), reset_on_new_game)
        .add_systems(OnExit(AppState::InGame), clear_overlay)
        .add_systems(
            FixedUpdate,
            (mark_removed, revive, run_end_flow)
                .chain()
                .in_set(GameSystems::Game)
                .after(super::player::tick)
                .run_if(in_state(AppState::InGame)),
        );
}
