//! Front-end screens: SEGA logo, title, character select and name entry
//! (docs/re/ui_screens.md). Each screen is a Bevy state with its own entities.
use super::{
    AppState, NewGame,
    dialog::{Dialog, PartyNames, Phase as DialogPhase},
    input::Joypad,
    player::CharacterClass,
    transition::{FADE_TICKS, ScreenFade},
    ui::ScreenRoot,
};
use super::audio::PlaySound;
use crate::{GameSystems, controller::Button};
use bevy::{prelude::*, sprite::Anchor};

/// Marks entities that belong to the current front-end screen.
#[derive(Component)]
pub struct ScreenEntity;

/// Generic per-screen timeline state.
#[derive(Resource, Default, Debug)]
pub struct ScreenTimer {
    pub frame: u32,
    pub step: u32,
    pub mark: u32,
}

fn screen_sprite(server: &AssetServer, root: Entity, path: &str, x: i32, y: i32, z: f32) -> impl Bundle {
    (
        Sprite::from_image(server.load(path.to_owned())),
        Anchor::TOP_LEFT,
        Transform::from_xyz(x as f32, -(y as f32), z),
        ChildOf(root),
        ScreenEntity,
    )
}

fn clear_screen(mut commands: Commands, q: Query<Entity, With<ScreenEntity>>, mut timer: ResMut<ScreenTimer>) {
    for e in &q {
        commands.entity(e).despawn();
    }
    *timer = ScreenTimer::default();
}

fn any_button(pad: &Joypad) -> bool {
    [Button::A, Button::B, Button::C, Button::Start].into_iter().any(|b| pad.pressed(b))
}

// ---------------------------------------------------------------------------
// SEGA logo
// ---------------------------------------------------------------------------

#[derive(Component)]
struct SegaLogo;

fn enter_sega(mut commands: Commands, server: Res<AssetServer>, roots: Query<Entity, With<ScreenRoot>>, mut fade: ResMut<ScreenFade>) {
    let Ok(root) = roots.single() else { return };
    fade.0 = 0.0;
    commands.spawn((screen_sprite(&server, root, "screens/sega/plane_a.png", 0, 0, 1.0), Visibility::Hidden));
    commands.spawn((screen_sprite(&server, root, "screens/sega/logo_cycle_00.png", 112, 96, 2.0), SegaLogo, Visibility::Hidden));
}

/// Logo visible from tick 41; a bright band sweeps once (21 steps x 4 ticks); hold 299 ticks; fade.
fn run_sega(
    server: Res<AssetServer>,
    pad: Res<Joypad>,
    mut timer: ResMut<ScreenTimer>,
    mut fade: ResMut<ScreenFade>,
    mut logo: Query<&mut Sprite, With<SegaLogo>>,
    mut vis: Query<&mut Visibility, With<ScreenEntity>>,
    mut next: ResMut<NextState<AppState>>,
) {
    timer.frame += 1;
    let t = timer.frame;
    const SHOW: u32 = 41;
    if t == SHOW {
        vis.iter_mut().for_each(|mut v| *v = Visibility::Inherited);
    }
    if t >= SHOW {
        let step = ((t - SHOW) / 4).min(20);
        if let Ok(mut s) = logo.single_mut() {
            s.image = server.load(format!("screens/sega/logo_cycle_{step:02}.png"));
        }
    }
    if timer.mark == 0 && (t >= SHOW + 299 || (t > SHOW && any_button(&pad))) {
        timer.mark = t;
    }
    if timer.mark > 0 {
        let n = t - timer.mark;
        fade.0 = n as f32 / FADE_TICKS as f32;
        if n >= FADE_TICKS {
            next.set(AppState::Title);
        }
    }
}

// ---------------------------------------------------------------------------
// Title
// ---------------------------------------------------------------------------

/// One horizontal line of a rippling title layer.
#[derive(Component)]
struct RippleLine {
    plane_b: bool,
    line: i32,
}

#[derive(Component)]
struct PressStart;

#[derive(Resource, Default, Debug)]
struct Ripple {
    /// Amplitude in 1/128 px (cartridge units).
    amplitude: i32,
    /// Table entries per scanline, 16.16.
    line_step: i32,
    /// Phase in table entries, 16.16.
    phase: i32,
    planes: u8,
    stage: TitleStage,
    stage_frame: u32,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
enum TitleStage {
    #[default]
    Intro,
    Idle,
    /// Left alone: the ripple grows back while the screen fades, then the attract demo starts.
    Outro,
    Leaving,
}

/// Idle title ticks before the outro (0x10DE8: 0xA8C frames).
const TITLE_IDLE_TICKS: u32 = 0xA8C;
/// Outro length (0x10EA0: 112 frames, one fade step every 5).
const TITLE_OUTRO_TICKS: u32 = 112;
/// Black ticks between the outro and the demo's fade-in. Cartridge (boot, no input): outro ends
/// at frame 3333, closing fade 0x2990 + demo room load until 3410, 26-frame fade-in, first demo tick
/// 3436. The remake's fade-in takes 42 ticks, and the cartridge also loses 24 lag frames to the
/// line-scroll ripple during the title intro and outro, which the remake doesn't have; the hold
/// absorbs both so that the first demo tick lands on the cartridge's frame.
const TITLE_TO_DEMO_HOLD: u32 = (3436 - 3333) - 42 + 24 - 1;

fn enter_title(
    mut commands: Commands,
    server: Res<AssetServer>,
    roots: Query<Entity, With<ScreenRoot>>,
    mut fade: ResMut<ScreenFade>,
    mut sound: MessageWriter<PlaySound>,
) {
    let Ok(root) = roots.single() else { return };
    fade.0 = 1.0;
    // Title music (main state 0, 0x3C1A).
    sound.write(PlaySound(0x81));
    for (path, plane_b, z) in [("screens/title/plane_b.png", true, 1.0), ("screens/title/plane_a.png", false, 2.0)] {
        let image: Handle<Image> = server.load(path);
        for line in 0..224 {
            commands.spawn((
                Sprite {
                    image: image.clone(),
                    rect: Some(Rect::new(0.0, line as f32, 320.0, line as f32 + 1.0)),
                    ..default()
                },
                Anchor::TOP_LEFT,
                Transform::from_xyz(0.0, -(line as f32), z),
                RippleLine { plane_b, line },
                ChildOf(root),
                ScreenEntity,
            ));
        }
    }
    commands.spawn((screen_sprite(&server, root, "screens/title/press_start.png", 88, 128, 3.0), PressStart, Visibility::Hidden));
    commands.insert_resource(Ripple {
        amplitude: 0x1500,
        line_step: 0x9_0000,
        phase: 0,
        planes: 3,
        stage: TitleStage::Intro,
        stage_frame: 0,
    });
}

/// Wave value for table entry `e` (period 128 entries), in pixels.
fn wave(amplitude: i32, e: i32) -> i32 {
    let a = amplitude as f32 / 128.0;
    (-(a) * (std::f32::consts::TAU * e as f32 / 128.0).cos()).round() as i32
}

#[allow(clippy::too_many_arguments)]
fn run_title(
    pads: Res<super::input::Joypads>,
    server: Res<AssetServer>,
    mut demo: ResMut<super::demo::AttractDemo>,
    mut new_game: ResMut<NewGame>,
    mut names: ResMut<PartyNames>,
    mut current: ResMut<super::room::CurrentRoom>,
    mut ripple: ResMut<Ripple>,
    mut fade: ResMut<ScreenFade>,
    mut lines: Query<(&RippleLine, &mut Transform)>,
    mut press: Query<&mut Visibility, With<PressStart>>,
    mut sound: MessageWriter<PlaySound>,
    mut next: ResMut<NextState<AppState>>,
) {
    // START on either pad (0x10D48 / 0x10E28).
    let pad = pads.any();
    let r = &mut *ripple;
    r.stage_frame += 1;
    match r.stage {
        TitleStage::Intro => {
            r.amplitude = (r.amplitude - 48).max(0);
            r.line_step -= 4096;
            // Palette fades in one component step every 5 ticks (21 steps).
            fade.0 = 1.0 - (r.stage_frame as f32 / 105.0).min(1.0);
            if r.amplitude == 0 || pad.pressed(Button::Start) {
                r.stage = TitleStage::Idle;
                r.stage_frame = 0;
                r.amplitude = 576;
                r.line_step = 0x3_0000;
                r.planes = 2;
                fade.0 = 0.0;
            }
        }
        TitleStage::Idle => {
            if pad.pressed(Button::Start) {
                sound.write(PlaySound(0x97));
                r.stage = TitleStage::Leaving;
                r.stage_frame = 0;
            } else if r.stage_frame >= TITLE_IDLE_TICKS {
                r.stage = TitleStage::Outro;
                r.stage_frame = 0;
                r.planes = 3;
                demo.prepare(&server, &mut new_game, &mut names, &mut current);
            }
        }
        TitleStage::Outro => {
            if pad.pressed(Button::Start) {
                // START during the outro jumps back to the idle title (0x10D5A).
                r.stage = TitleStage::Idle;
                r.stage_frame = 0;
                r.amplitude = 576;
                r.line_step = 0x3_0000;
                r.planes = 2;
                fade.0 = 0.0;
            } else if r.stage_frame <= TITLE_OUTRO_TICKS {
                r.amplitude += 48;
                r.line_step += 4096;
                fade.0 = (r.stage_frame as f32 / 105.0).min(1.0);
            } else {
                fade.0 = 1.0;
                if r.stage_frame >= TITLE_OUTRO_TICKS + TITLE_TO_DEMO_HOLD && super::demo::AttractDemo::ready(&server, &current) {
                    super::demo::begin(&mut demo, &mut next);
                }
            }
        }
        TitleStage::Leaving => {
            // Wait 61 ticks, then the standard fade.
            if r.stage_frame > 61 {
                fade.0 = ((r.stage_frame - 61) as f32 / FADE_TICKS as f32).min(1.0);
                if r.stage_frame - 61 >= FADE_TICKS {
                    next.set(AppState::CharSelect);
                }
            }
        }
    }
    r.phase = r.phase.wrapping_add(0x8_0000);
    for (l, mut tf) in &mut lines {
        let active = if l.plane_b { r.planes & 2 != 0 } else { r.planes & 1 != 0 };
        let dx = if active {
            let e = ((r.phase as i64 + l.line as i64 * r.line_step as i64) >> 16) as i32;
            wave(r.amplitude, e.rem_euclid(128))
        } else {
            0
        };
        tf.translation.x = dx as f32;
    }
    // PRESS START: 60 ticks visible, 10 hidden, only once the title has settled.
    if let Ok(mut v) = press.single_mut() {
        let on = r.stage == TitleStage::Idle && r.stage_frame % 70 < 60
            || r.stage == TitleStage::Leaving && (r.stage_frame / 4) % 2 == 0;
        *v = if on { Visibility::Inherited } else { Visibility::Hidden };
    }
}

// ---------------------------------------------------------------------------
// Character select
// ---------------------------------------------------------------------------

#[derive(Component)]
struct SelectFrame;

#[derive(Resource, Debug, Default)]
struct Select {
    stage: SelectStage,
    players_cursor: usize,
    class: usize,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
enum SelectStage {
    #[default]
    FadeIn,
    Menu,
    PlayersMessage,
    Choosing,
    Leaving,
}

const MENU_SHAPE: usize = 11;
const MESSAGE_SHAPE: usize = 12;
const TEXT_ONE_PLAYER: u32 = 132;
const TEXT_TWO_PLAYERS: u32 = 133;
const TEXT_FIGHTER: u32 = 134;
const TEXT_MAGE: u32 = 135;

fn enter_select(
    mut commands: Commands,
    server: Res<AssetServer>,
    roots: Query<Entity, With<ScreenRoot>>,
    mut fade: ResMut<ScreenFade>,
    mut sound: MessageWriter<PlaySound>,
) {
    let Ok(root) = roots.single() else { return };
    fade.0 = 1.0;
    sound.write(PlaySound(0x82));
    commands.spawn(screen_sprite(&server, root, "screens/charselect/plane_a.png", 0, 0, 1.0));
    // Object 0xFF0DC0 at VDP (0xD0, 0xB8) = screen (80, 56); `run_select` moves it to x 184 for the Mage.
    commands.spawn((
        screen_sprite(&server, root, "screens/charselect/select_frame_0.png", 80, 56, 2.0),
        SelectFrame,
        Visibility::Hidden,
    ));
    commands.spawn((
        screen_sprite(&server, root, "screens/charselect/arrow.png", 128, 160, super::coords::z::HUD + 2.0),
        MenuArrow,
        Visibility::Hidden,
    ));
    commands.insert_resource(Select::default());
}

#[derive(Component)]
struct MenuArrow;

#[allow(clippy::too_many_arguments)]
fn run_select(
    server: Res<AssetServer>,
    pad: Res<Joypad>,
    mut timer: ResMut<ScreenTimer>,
    mut sel: ResMut<Select>,
    mut fade: ResMut<ScreenFade>,
    mut dialog: ResMut<Dialog>,
    mut frame: Query<(&mut Sprite, &mut Transform, &mut Visibility), (With<SelectFrame>, Without<MenuArrow>)>,
    mut arrow: Query<(&mut Transform, &mut Visibility), (With<MenuArrow>, Without<SelectFrame>)>,
    mut new_game: ResMut<NewGame>,
    controllers: Res<crate::controller::Controllers>,
    mut sound: MessageWriter<PlaySound>,
    mut next: ResMut<NextState<AppState>>,
) {
    timer.frame += 1;
    timer.step += 1;
    // Cursor moves and confirmations click (B4, 0x656C..0x6718).
    let mut click = || {
        sound.write(PlaySound(0xB4));
    };
    match sel.stage {
        SelectStage::FadeIn => {
            fade.0 = 1.0 - (timer.step as f32 / FADE_TICKS as f32).min(1.0);
            if timer.step >= FADE_TICKS + 20 {
                dialog.open_static(MENU_SHAPE, vec![" 1PLAYER".into(), " 2PLAYERS".into()]);
                sel.stage = SelectStage::Menu;
                timer.step = 0;
            }
        }
        SelectStage::Menu => {
            // 0x6530: B confirms; with a pad in port 2 UP picks 1PLAYER and DOWN 2PLAYERS, without
            // one any other press puts the cursor back on 1PLAYER.
            let b = pad.pressed(Button::B);
            let before = sel.players_cursor;
            if !b && controllers.connected(1) {
                if pad.pressed(Button::Up) {
                    sel.players_cursor = 0;
                } else if pad.pressed(Button::Down) {
                    sel.players_cursor = 1;
                }
            } else if !b && Button::ALL.into_iter().any(|x| pad.pressed(x)) {
                sel.players_cursor = 0;
            }
            if sel.players_cursor != before {
                click();
            }
            if b && dialog.phase() == Some(DialogPhase::Halted) {
                click();
                // 0xFF0A40 = cursor (0 one player, 1 two players).
                new_game.players = sel.players_cursor + 1;
                dialog.close();
                dialog.open(if sel.players_cursor == 1 { TEXT_TWO_PLAYERS } else { TEXT_ONE_PLAYER }, MESSAGE_SHAPE);
                sel.stage = SelectStage::PlayersMessage;
                timer.step = 0;
            }
        }
        SelectStage::PlayersMessage => {
            if dialog.phase() == Some(DialogPhase::Halted) && dialog.active_shape() == Some(MESSAGE_SHAPE) && timer.step > 60 {
                dialog.replace_text(TEXT_FIGHTER);
                sel.stage = SelectStage::Choosing;
                sel.class = 0;
            }
        }
        SelectStage::Choosing => {
            // LEFT picks the Fighter, RIGHT the Mage (0x66BE / 0x66E4).
            let before = sel.class;
            if pad.pressed(Button::Left) {
                sel.class = 0;
            } else if pad.pressed(Button::Right) {
                sel.class = 1;
            }
            if sel.class != before {
                click();
                dialog.replace_text(if sel.class == 0 { TEXT_FIGHTER } else { TEXT_MAGE });
            }
            if pad.pressed(Button::B) {
                click();
                // Player 1 chooses; player 2 plays the other class (0xFF0A44 / 0xFF0A46).
                new_game.class = if sel.class == 0 { CharacterClass::Fighter } else { CharacterClass::Mage };
                dialog.close();
                sel.stage = SelectStage::Leaving;
                timer.step = 0;
            }
        }
        SelectStage::Leaving => {
            fade.0 = (timer.step as f32 / FADE_TICKS as f32).min(1.0);
            if timer.step >= FADE_TICKS && !dialog.is_open() {
                next.set(AppState::NameEntry);
            }
        }
    }
    // 1PLAYER / 2PLAYERS cursor (window shape 11 at (112,152), cursor column 2).
    if let Ok((mut tf, mut v)) = arrow.single_mut() {
        let shown = sel.stage == SelectStage::Menu && dialog.phase() == Some(DialogPhase::Halted);
        *v = if shown { Visibility::Inherited } else { Visibility::Hidden };
        tf.translation.y = -((160 + 8 * sel.players_cursor) as f32);
    }
    // Selection frame around the chosen portrait; its colours rotate every 4 ticks.
    if let Ok((mut s, mut tf, mut v)) = frame.single_mut() {
        let shown = matches!(sel.stage, SelectStage::Choosing | SelectStage::Leaving);
        *v = if shown { Visibility::Inherited } else { Visibility::Hidden };
        let x = if sel.class == 0 { 80 } else { 184 };
        tf.translation.x = x as f32;
        s.image = server.load(format!("screens/charselect/select_frame_{}.png", (timer.frame / 4) % 5));
    }
}

// ---------------------------------------------------------------------------
// Name entry
// ---------------------------------------------------------------------------

#[derive(Component)]
struct NameCursor;

#[derive(Component)]
struct NameLetters;

/// One player's name being typed (cartridge cursor objects 0xFF0DC0 / 0xFF0F40).
#[derive(Debug, Default, Clone)]
struct NameSlot {
    col: i32,
    row: i32,
    name: Vec<u8>,
    done: bool,
}

#[derive(Resource, Debug, Default)]
struct NameEntry {
    slots: Vec<NameSlot>,
    leaving: bool,
}

/// Letter grid (rows of 6); the last row only has END at column 4.
const GRID: [[u8; 6]; 5] = [
    *b"ABCDEF",
    *b"GHIJKL",
    *b"MNOPQR",
    *b"STUVWX",
    [b'Y', b'Z', 0x3A, 0x3C, 0x3D, 0x3F],
];
const END_ROW: i32 = 5;
const END_COL: i32 = 4;
/// Name boxes (letters at screen x, y = 64): P1 nametable 0xC412, P2 0xC436.
const NAME_X: [i32; 2] = [72, 216];
/// Starting cursor column: P1 on A, P2 on F (grid pointers 0x720F / 0x7214).
const START_COL: [i32; 2] = [0, 5];

#[derive(Component)]
struct NameSlotIndex(usize);

fn enter_name(
    mut commands: Commands,
    server: Res<AssetServer>,
    roots: Query<Entity, With<ScreenRoot>>,
    mut fade: ResMut<ScreenFade>,
    new_game: Res<NewGame>,
) {
    let Ok(root) = roots.single() else { return };
    fade.0 = 1.0;
    let players = new_game.players.clamp(1, 2);
    let layout = if players == 2 { "2p" } else { "1p" };
    commands.spawn(screen_sprite(&server, root, &format!("screens/nameentry/plane_b_{layout}.png"), 0, 0, 1.0));
    commands.spawn(screen_sprite(&server, root, &format!("screens/nameentry/plane_a_{layout}.png"), 0, 0, 2.0));
    for slot in 0..players {
        let cursor = format!("screens/nameentry/cursor_p{}.png", slot + 1);
        commands.spawn((
            screen_sprite(&server, root, &cursor, 104 + START_COL[slot] * 16, 95, 3.0 - slot as f32 * 0.1),
            NameCursor,
            NameSlotIndex(slot),
        ));
        let grid = super::ui::spawn_grid(&mut commands, root, NAME_X[slot], 64, 3, 1, 2.5);
        commands.entity(grid).insert((NameLetters, NameSlotIndex(slot), ScreenEntity));
    }
    let slots = (0..players).map(|slot| NameSlot { col: START_COL[slot], ..default() }).collect();
    commands.insert_resource(NameEntry { slots, leaving: false });
}

/// Three name bytes as stored by the cartridge (unused letters are 0x3A blanks).
fn padded(name: &[u8]) -> [u8; 3] {
    std::array::from_fn(|i| name.get(i).copied().unwrap_or(0x3A))
}

fn name_string(name: &[u8]) -> String {
    name.iter().map(|&b| super::dialog::byte_char(b)).collect()
}

/// Cursor movement and typing for one player (cartridge 0x6978). Returns true when the click (B4)
/// sounds: the cursor jumping to END after the third letter, and a finished name.
fn name_input(e: &mut NameSlot, pad: &Joypad, other_done: Option<[u8; 3]>, class: CharacterClass) -> bool {
    let in_end_row = e.row == END_ROW;
    if pad.pressed(Button::Left) {
        e.col = if in_end_row { END_COL } else { (e.col - 1).rem_euclid(6) };
    }
    if pad.pressed(Button::Right) {
        e.col = if in_end_row { END_COL } else { (e.col + 1).rem_euclid(6) };
    }
    if pad.pressed(Button::Up) {
        e.row = (e.row - 1).rem_euclid(6);
        if e.row == END_ROW {
            e.col = END_COL;
        }
    }
    if pad.pressed(Button::Down) {
        e.row = (e.row + 1).rem_euclid(6);
        if e.row == END_ROW {
            e.col = END_COL;
        }
    }
    if pad.pressed(Button::A) || pad.pressed(Button::C) {
        e.name.pop();
    }
    if pad.pressed(Button::B) {
        if e.row == END_ROW {
            // Two players may not share a name: END then acts like delete (0x6B22).
            if other_done.is_some_and(|o| o == padded(&e.name)) {
                e.name.pop();
                return false;
            }
            if e.name.iter().all(|&b| b == 0x3A) {
                e.name = match class {
                    CharacterClass::Fighter => b"FIG".to_vec(),
                    CharacterClass::Mage => b"MAG".to_vec(),
                };
            }
            e.done = true;
            return true;
        } else if e.name.len() < 3 {
            e.name.push(GRID[e.row as usize][e.col as usize]);
            if e.name.len() == 3 {
                e.row = END_ROW;
                e.col = END_COL;
                return true;
            }
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn run_name(
    pads: Res<super::input::Joypads>,
    mut timer: ResMut<ScreenTimer>,
    mut entry: ResMut<NameEntry>,
    mut fade: ResMut<ScreenFade>,
    mut names: ResMut<PartyNames>,
    new_game: Res<NewGame>,
    ui: Res<super::ui::UiAssets>,
    fonts: Res<Assets<super::data::FontDef>>,
    mut cursor: Query<(&mut Transform, &mut Visibility, &NameSlotIndex), With<NameCursor>>,
    letters: Query<(&super::ui::CellGrid, &NameSlotIndex), With<NameLetters>>,
    mut sprites: Query<&mut Sprite>,
    mut sound: MessageWriter<PlaySound>,
    mut next: ResMut<NextState<AppState>>,
) {
    timer.frame += 1;
    if entry.leaving {
        timer.step += 1;
        fade.0 = (timer.step as f32 / FADE_TICKS as f32).min(1.0);
        if timer.step >= FADE_TICKS {
            next.set(AppState::InGame);
        }
        return;
    }
    fade.0 = 1.0 - (timer.frame as f32 / FADE_TICKS as f32).min(1.0);
    for i in 0..entry.slots.len() {
        if entry.slots[i].done {
            continue;
        }
        let other_done = entry.slots.iter().enumerate().find(|(j, o)| *j != i && o.done).map(|(_, o)| padded(&o.name));
        if name_input(&mut entry.slots[i], &pads.get(i), other_done, new_game.class_of(i)) {
            sound.write(PlaySound(0xB4));
        }
    }
    if entry.slots.iter().all(|e| e.done) {
        let mut all: Vec<String> = entry
            .slots
            .iter()
            .map(|e| {
                let mut n = name_string(&e.name);
                while n.len() < 3 {
                    n.push(' ');
                }
                n
            })
            .collect();
        names.p1 = all.remove(0);
        names.p2 = all.pop();
        entry.leaving = true;
        timer.step = 0;
    }
    for (mut tf, mut vis, idx) in &mut cursor {
        let Some(e) = entry.slots.get(idx.0) else { continue };
        tf.translation.x = (104 + e.col * 16) as f32;
        tf.translation.y = -((95 + e.row * 16) as f32);
        // A finished player's cursor disappears (object cleared, 0x6BA0).
        *vis = if e.done { Visibility::Hidden } else { Visibility::Inherited };
    }
    let Some(font) = fonts.get(&ui.font) else { return };
    for (grid, idx) in &letters {
        let Some(e) = entry.slots.get(idx.0) else { continue };
        for i in 0..3 {
            let Ok(mut s) = sprites.get_mut(grid.cells[i]) else { continue };
            match e.name.get(i) {
                Some(&b) => super::ui::set_cell(font, &mut s, &super::ui::Cell::byte(b)),
                None => s.rect = Some(Rect::new(0.0, 0.0, 0.0, 0.0)),
            }
        }
    }
}

pub fn plugin(app: &mut App) {
    app.init_resource::<ScreenTimer>()
        .add_systems(OnEnter(AppState::Sega), (clear_screen, enter_sega).chain())
        .add_systems(OnEnter(AppState::Title), (clear_screen, enter_title).chain())
        .add_systems(OnEnter(AppState::CharSelect), (clear_screen, enter_select).chain())
        .add_systems(OnEnter(AppState::NameEntry), (clear_screen, enter_name).chain())
        .add_systems(OnEnter(AppState::InGame), clear_screen)
        .add_systems(OnEnter(AppState::Ending), clear_screen)
        .add_systems(
            FixedUpdate,
            (
                run_sega.run_if(in_state(AppState::Sega)),
                run_title.run_if(in_state(AppState::Title)),
                run_select.run_if(in_state(AppState::CharSelect)),
                run_name.run_if(in_state(AppState::NameEntry)),
            )
                .in_set(GameSystems::Game),
        );
}
