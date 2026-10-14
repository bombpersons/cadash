//! Attract demo (cartridge main state 1, 0x3C2C; docs/re/attract_demo.md).
//!
//! When the title is left alone its outro leads here: a two-player game (FIG the Fighter and MAG
//! the Mage) starts at room 1's door #1, i.e. room 2 at (2464,192), and both pads are replayed from
//! the recorded streams `attract_p1` / `attract_p2`. A real START on either pad returns to the
//! title; when a stream runs out the screen fades and the cartridge resets to the SEGA logo.
use super::{
    AppState, GameDataRef, NewGame,
    camera::CameraPos,
    data::ExitDest,
    dialog::{PartyNames, StoryFlags},
    input::DemoInput,
    lifecycle::Lives,
    objects::{PersistFlags, ops::Effects},
    player::{CharacterClass, Facing, Player, Stats},
    room::{self, CurrentRoom},
    room_events::WorldFreeze,
    transition::{FADE_TICKS, FadeTo, Phase, ScreenFade, Transition},
};
use crate::{
    GameSystems,
    controller::{Button, Controllers},
};
use bevy::prelude::*;

/// Demo room: room 1's door #1 (entrance table 0x1388A, record 1 of room 1 = 0x138FE + 14).
const ROOM: usize = 2;
const START: (i32, i32) = (2464, 192);
const CAMERA: [i32; 2] = [2240, 64];
/// Music of the entrance record the demo starts from.
const MUSIC: u32 = 0x87;
/// EXP given to P1 / P2 (0x46F0): both start at level 2.
const EXP: [u32; 2] = [50, 70];
/// Stream ticks consumed before the first gameplay tick: the replay runs from the vertical
/// interrupt and also advances during the room load (4 polls) and the fade-in (26 polls).
const PREROLL: u32 = 30;
/// Value of the tick counter 0xFF0240 on the first demo tick (boot with no input).
const FIRST_TICK: u64 = 3284;

/// State of the attract demo. While `active`, gameplay runs from [`DemoInput`].
#[derive(Resource, Default)]
pub struct AttractDemo {
    pub active: bool,
    /// The replay has been started (on the first gameplay tick after the fade-in).
    started: bool,
    /// Ticks since the demo ended (stream exhausted or START), while fading out.
    leaving: Option<(u32, AppState)>,
    /// Settings of the real game, restored when the demo ends.
    saved: Option<(NewGame, PartyNames)>,
}

impl AttractDemo {
    /// Prepares the demo game (0x3C2C) and starts loading its room; the title switches to
    /// [`AppState::InGame`] once [`AttractDemo::ready`] says the room is resident.
    pub fn prepare(&mut self, server: &AssetServer, new_game: &mut NewGame, names: &mut PartyNames, current: &mut CurrentRoom) {
        if self.saved.is_none() {
            self.saved = Some((*new_game, names.clone()));
        }
        *new_game = NewGame {
            room: ROOM,
            pos: super::coords::WorldPos::from_px(START.0, START.1),
            facing: Facing::Left,
            class: CharacterClass::Fighter,
            players: 2,
            custom_pos: true,
            hp: None,
            gold: 0,
            gear: Default::default(),
        };
        // 0xFF0A4E / 0xFF0A51.
        *names = PartyNames { p1: "FIG".into(), p2: Some("MAG".into()) };
        if current.index != ROOM {
            *current = CurrentRoom { index: ROOM, def: server.load(room::room_path(ROOM)) };
        }
    }
    pub fn ready(server: &AssetServer, current: &CurrentRoom) -> bool {
        current.index == ROOM && server.is_loaded_with_dependencies(&current.def)
    }
}

/// Starts the demo game: called by the title when it hands over to gameplay.
pub fn begin(demo: &mut AttractDemo, next: &mut NextState<AppState>) {
    demo.active = true;
    demo.started = false;
    demo.leaving = None;
    next.set(AppState::InGame);
}

/// Runs after `start_game`: entrance placement, levels, no credits, a fresh session and a
/// gameplay freeze during the fade-in (the cartridge fades in before its main loop starts).
#[allow(clippy::too_many_arguments)]
fn setup_demo(
    mut commands: Commands,
    demo: Res<AttractDemo>,
    data: GameDataRef,
    mut cam: ResMut<CameraPos>,
    mut lives: ResMut<Lives>,
    mut sound: MessageWriter<super::audio::PlaySound>,
    mut players: Query<(&Player, &CharacterClass, &mut super::coords::WorldPos, &mut Stats)>,
) {
    if !demo.active {
        return;
    }
    sound.write(super::audio::PlaySound(MUSIC));
    // Credits stay 0 (0x4592 is skipped in demo mode).
    lives.0 = 0;
    cam.x = CAMERA[0];
    cam.y = CAMERA[1];
    let Some(d) = data.get() else { return };
    for (p, class, mut pos, mut stats) in &mut players {
        // Both players stand on the entrance spot (0x48B8).
        *pos = super::coords::WorldPos::from_px(START.0, START.1);
        let levels = match class {
            CharacterClass::Fighter => &d.levels.fighter,
            CharacterClass::Mage => &d.levels.mage,
        };
        let exp = EXP[p.slot.min(1)];
        let level = levels.iter().rposition(|l| l.exp <= exp).unwrap_or(0);
        *stats = Stats::from_level(level, &levels[level]);
        stats.exp = exp;
    }
    commands.remove_resource::<FadeTo>();
    let dest = ExitDest {
        room: ROOM,
        x: START.0,
        y: START.1,
        facing: "left".into(),
        camera: CAMERA,
        music: 0,
        sound: None,
    };
    commands.insert_resource(Transition { dest, phase: Phase::FadeIn(0) });
}

/// Starts the replay on the first gameplay tick; after the demo ended, waits for the fade and
/// leaves (cartridge 0xC06: 0xFF0A72 = 4 / 3, then fade 0x2990).
#[allow(clippy::too_many_arguments)]
fn run_demo(
    mut demo: ResMut<AttractDemo>,
    mut input: ResMut<DemoInput>,
    mut clock: ResMut<crate::GameClock>,
    transition: Option<Res<Transition>>,
    data: GameDataRef,
    state: Res<State<AppState>>,
    fade: Res<ScreenFade>,
    mut next: ResMut<NextState<AppState>>,
) {
    if !demo.active || *state.get() != AppState::InGame {
        return;
    }
    if let Some((t, to)) = demo.leaving {
        // The replay keeps running (idle once exhausted) while the screen fades.
        demo.leaving = Some((t + 1, to));
        if t + 1 >= FADE_TICKS && fade.0 >= 1.0 {
            input.stop();
            next.set(to);
        }
        return;
    }
    if !demo.started {
        if transition.is_some() {
            return;
        }
        let Some(d) = data.get() else { return };
        let stream = |k: &str| d.demos.get(k).cloned().unwrap_or_default();
        input.start_ports(vec![stream("attract_p1"), stream("attract_p2")], PREROLL);
        // The cartridge's tick counter 0xFF0240 reads 3284 on the first demo tick; odd/even-tick
        // logic (melee hit tests, contact damage) depends on it.
        clock.tick = FIRST_TICK;
        demo.started = true;
    }
}

/// Right after the pads are read (cartridge 0xC06 / 0x40DA): a real START (not the replayed one)
/// on either pad leaves for the title, an exhausted stream for the SEGA logo. From this tick on the
/// world stands still while the screen fades (0x4208 → 0x2990).
fn check_demo_end(
    mut commands: Commands,
    mut demo: ResMut<AttractDemo>,
    input: Res<DemoInput>,
    controllers: Res<Controllers>,
    mut freeze: ResMut<WorldFreeze>,
    state: Res<State<AppState>>,
) {
    if !demo.active || !demo.started || demo.leaving.is_some() || *state.get() != AppState::InGame {
        return;
    }
    let start = controllers.ports.iter().any(|p| p.pressed(Button::Start));
    let to = if start {
        AppState::Title
    } else if input.finished() {
        AppState::Sega
    } else {
        return;
    };
    demo.leaving = Some((0, to));
    freeze.0 = true;
    commands.insert_resource(FadeTo { target: 1.0, ticks: FADE_TICKS });
}

/// Leaving gameplay (to the title, the SEGA logo after a demo, or after a game over): removes the
/// room, actors and HUD, restores the real game's settings and the room it starts in, and clears
/// the session's progress.
#[allow(clippy::too_many_arguments)]
fn end_demo(
    mut commands: Commands,
    server: Res<AssetServer>,
    mut demo: ResMut<AttractDemo>,
    mut input: ResMut<DemoInput>,
    mut new_game: ResMut<NewGame>,
    mut names: ResMut<PartyNames>,
    mut current: ResMut<CurrentRoom>,
    mut freeze: ResMut<WorldFreeze>,
    mut obj_world: ResMut<super::objects::ObjectWorld>,
    old_room: Query<Entity, With<room::RoomEntity>>,
    old_actors: Query<Entity, Or<(With<Player>, With<super::ui::Hud>)>>,
) {
    // Whatever game was running (demo, or a game that ended in GAME OVER) leaves the screen.
    room::despawn_room(&mut commands, &old_room);
    for e in &old_actors {
        commands.entity(e).despawn();
    }
    super::objects::clear_objects(&mut obj_world);
    let was_active = demo.active;
    if let Some((g, n)) = demo.saved.take() {
        *new_game = g;
        *names = n;
    }
    *demo = AttractDemo::default();
    if was_active {
        input.stop();
        freeze.0 = false;
    }
    // A new game (or the next demo) starts from a clean slate (the cartridge clears RAM, 0x3D80).
    commands.insert_resource(StoryFlags::default());
    commands.insert_resource(PersistFlags::default());
    commands.insert_resource(Effects::default());
    if current.index != new_game.room {
        *current = CurrentRoom { index: new_game.room, def: server.load(room::room_path(new_game.room)) };
    }
}

/// The demo game cannot be paused (0x2B52).
pub fn not_in_demo(demo: Res<AttractDemo>) -> bool {
    !demo.active
}

pub fn plugin(app: &mut App) {
    app.init_resource::<AttractDemo>()
        .add_systems(OnEnter(AppState::InGame), setup_demo.after(super::start_game))
        .add_systems(OnEnter(AppState::Sega), end_demo)
        .add_systems(OnEnter(AppState::Title), end_demo)
        .add_systems(
            FixedUpdate,
            (run_demo.before(super::input::sample), check_demo_end.after(super::input::sample))
                .in_set(GameSystems::Input)
                .after(crate::begin_tick)
                .run_if(|c: Res<crate::GameClock>| c.ticking),
        );
}
