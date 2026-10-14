//! Text windows and the cartridge's text engine (docs/re/ui_rpg.md §3, docs/re/shops.md).
//!
//! [`Windows`] owns any number of on-screen windows (shapes from the cartridge's window table).
//! A window opens two cells per tick (width first, then height) and closes in reverse. A window can
//! carry a [`TextRun`]: text from the token graph (`text/dialogue.text.json`) with flags, branches,
//! name inserts, pages and YES/NO stops, typed one glyph every 3 ticks in the NPC window (shape 0)
//! or every 2 ticks elsewhere.
//!
//! [`Dialog`] is the single "conversation" window used by NPCs, pickups and scripted messages;
//! gameplay pauses while it (or any modal window) is open.
use super::{
    data::{DialogueDef, FontDef, Token, WindowShape},
    input::Joypad,
    ui::{Cell, CellGrid, ScreenRoot, UiAssets, glyph_byte, spawn_grid},
};
use crate::{GameSystems, controller::Button};
use bevy::prelude::*;
use std::collections::HashMap;

/// Story and item flags, keyed by the cartridge RAM address they live at (e.g. 0xFF0AD3 = silver
/// key). Text scripts, item pickups and object scripts share them.
#[derive(Resource, Default, Debug)]
pub struct StoryFlags(pub HashMap<u32, u8>);

fn flag_addr(s: &str) -> u32 {
    u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap_or(0)
}

/// Names used by text inserts.
#[derive(Resource, Debug, Clone)]
pub struct PartyNames {
    pub p1: String,
    pub p2: Option<String>,
}

impl Default for PartyNames {
    fn default() -> Self {
        Self { p1: "AAA".into(), p2: None }
    }
}

/// Converts a glyph byte back to the character it shows.
pub fn byte_char(b: u8) -> char {
    match b {
        0x3A => ' ',
        0x3B => ',',
        0x3C => '.',
        0x3D => '!',
        0x3E => '\'',
        0x3F => '?',
        0x5B => '/',
        _ => b as char,
    }
}

// ---------------------------------------------------------------------------
// Text runs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunPhase {
    Printing,
    WaitPage,
    WaitEnd,
    /// Stopped at a YES/NO marker; the owner decides what happens next.
    Halted,
    Done,
}

/// Text being typed into a window.
#[derive(Debug, Clone)]
pub struct TextRun {
    pub start_id: u32,
    pub phase: RunPhase,
    node: Option<String>,
    token: usize,
    pending: Vec<u8>,
    col: usize,
    row: usize,
    /// Text area origin (cells inside the window) and size.
    pub origin: (usize, usize),
    pub area: (usize, usize),
    text: Vec<u8>,
    arrow: Option<(usize, usize)>,
    timer: u32,
    wait: u32,
    /// Ticks per glyph.
    pub interval: u32,
    /// Holding B prints a glyph every tick.
    pub b_speedup: bool,
    /// Pages and endings continue automatically after these many ticks instead of waiting for B.
    pub auto_page: Option<u32>,
    pub auto_end: Option<u32>,
    pub speaker: usize,
    /// Value for the {NUM} insert (prices).
    pub number: u32,
    /// Every typed glyph except spaces clicks (sound B2), except in window shape 1 (0x2C69A).
    pub click: bool,
}

impl TextRun {
    pub fn new(id: u32, origin: (usize, usize), area: (usize, usize)) -> Self {
        Self {
            start_id: id,
            phase: RunPhase::Printing,
            node: None,
            token: 0,
            pending: vec![],
            col: 0,
            row: 0,
            origin,
            area,
            text: vec![0x3A; area.0 * area.1],
            arrow: None,
            timer: 0,
            wait: 0,
            interval: 2,
            b_speedup: false,
            auto_page: None,
            auto_end: None,
            speaker: 0,
            number: 0,
            click: true,
        }
    }
    /// The conversation-window configuration for a window shape (cartridge 0x2C4B6).
    pub fn for_shape(id: u32, shape: usize, w: usize, h: usize) -> Self {
        let mut r = Self::new(id, (2, 1), (w.saturating_sub(4), h.saturating_sub(2)));
        r.click = shape != 1;
        if shape == 0 {
            r.interval = 3;
            r.b_speedup = true;
        } else {
            r.interval = 2;
            r.b_speedup = true;
            r.auto_page = Some(0x3D);
            r.auto_end = Some(0x51);
        }
        r
    }
    pub fn clear(&mut self) {
        self.text.iter_mut().for_each(|c| *c = 0x3A);
        self.col = 0;
        self.row = 0;
        self.arrow = None;
    }
    pub fn restart(&mut self, id: u32) {
        self.start_id = id;
        self.node = None;
        self.token = 0;
        self.pending.clear();
        self.clear();
        self.phase = RunPhase::Printing;
    }
    /// Glyph at a text-area cell, or the page arrow.
    fn cell(&self, c: usize, r: usize) -> Option<Cell> {
        if self.arrow == Some((c, r)) {
            return Some(Cell::named("PROMPT_ARROW"));
        }
        (c < self.area.0 && r < self.area.1).then(|| Cell::byte(self.text[r * self.area.0 + c]))
    }

    fn next_glyph(&mut self, def: &DialogueDef, flags: &mut StoryFlags, names: &PartyNames) -> Option<u8> {
        loop {
            if !self.pending.is_empty() {
                return Some(self.pending.remove(0));
            }
            if self.node.is_none() {
                self.node = def.entries.get(&self.start_id.to_string()).cloned();
                self.token = 0;
                if self.node.is_none() {
                    self.phase = RunPhase::WaitEnd;
                    return None;
                }
            }
            let addr = self.node.clone().unwrap();
            let Some(node) = def.nodes.get(&addr) else {
                self.phase = RunPhase::WaitEnd;
                return None;
            };
            let Some(tok) = node.tokens.get(self.token) else {
                match &node.next {
                    Some(n) => {
                        self.node = Some(n.clone());
                        self.token = 0;
                        continue;
                    }
                    None => {
                        self.phase = RunPhase::WaitEnd;
                        return None;
                    }
                }
            };
            self.token += 1;
            match tok {
                Token::Text { s } => self.pending.extend(s.chars().map(glyph_byte)),
                Token::Nl => {
                    self.row += 1;
                    self.col = 0;
                }
                Token::P1Name => self.pending.extend(names.p1.chars().map(glyph_byte)),
                Token::P2Name => self.pending.extend(names.p2.clone().unwrap_or_default().chars().map(glyph_byte)),
                Token::SpeakerName | Token::PlayerName => {
                    let n = if self.speaker == 0 { names.p1.clone() } else { names.p2.clone().unwrap_or_default() };
                    self.pending.extend(n.chars().map(glyph_byte));
                }
                Token::Page => {
                    if self.auto_page.is_none() {
                        self.arrow = Some((10, self.row + 1));
                    }
                    self.phase = RunPhase::WaitPage;
                    self.wait = self.auto_page.unwrap_or(0);
                    return None;
                }
                Token::End => {
                    self.phase = RunPhase::WaitEnd;
                    self.wait = self.auto_end.unwrap_or(0);
                    return None;
                }
                Token::YesNo => {
                    self.phase = RunPhase::Halted;
                    return None;
                }
                Token::Cont => {}
                Token::Goto { target } => {
                    self.node = Some(target.clone());
                    self.token = 0;
                }
                Token::If2p { target } => {
                    if names.p2.is_some() {
                        self.node = Some(target.clone());
                        self.token = 0;
                    }
                }
                Token::IfFlag { flag, target } => {
                    if flags.0.get(&flag_addr(flag)).copied().unwrap_or(0) != 0 {
                        self.node = Some(target.clone());
                        self.token = 0;
                    }
                }
                Token::IncFlag { flag } => {
                    let v = flags.0.entry(flag_addr(flag)).or_default();
                    *v = v.wrapping_add(1);
                }
                Token::ClrFlag { flag } => {
                    flags.0.insert(flag_addr(flag), 0);
                }
                Token::SetFlag { flag, value } => {
                    flags.0.insert(flag_addr(flag), *value);
                }
                Token::Number => self.pending.extend(format!("{:>5}", self.number).chars().map(glyph_byte)),
                Token::Other | Token::IfCheat { .. } | Token::IfNoLives { .. } => {}
            }
        }
    }

    /// Advances the run by one tick.
    /// Advances the text by one tick; returns true when a glyph that clicks was typed.
    pub fn tick(&mut self, def: &DialogueDef, flags: &mut StoryFlags, names: &PartyNames, pad: &Joypad) -> bool {
        let new_b = pad.pressed(Button::B);
        let mut clicked = false;
        match self.phase {
            RunPhase::Printing => {
                if self.timer > 0 {
                    self.timer -= 1;
                }
                if self.timer == 0 {
                    if let Some(g) = self.next_glyph(def, flags, names) {
                        if self.col < self.area.0 && self.row < self.area.1 {
                            self.text[self.row * self.area.0 + self.col] = g;
                        }
                        self.col += 1;
                        self.timer = if self.b_speedup && pad.held(Button::B) { 1 } else { self.interval };
                        clicked = self.click && g != 0x3A;
                    }
                }
            }
            RunPhase::WaitPage => {
                let go = match self.auto_page {
                    Some(_) => {
                        self.wait = self.wait.saturating_sub(1);
                        self.wait == 0
                    }
                    None => new_b,
                };
                if go {
                    self.clear();
                    self.phase = RunPhase::Printing;
                }
            }
            RunPhase::WaitEnd => {
                let go = match self.auto_end {
                    Some(_) => {
                        self.wait = self.wait.saturating_sub(1);
                        self.wait == 0
                    }
                    None => new_b,
                };
                if go {
                    self.phase = RunPhase::Done;
                }
            }
            RunPhase::Halted | RunPhase::Done => {}
        }
        clicked
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinState {
    Opening,
    Open,
    Closing,
    Closed,
}

/// Handle to a window in [`Windows`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct WindowId(pub u32);

#[derive(Debug, Clone)]
pub struct Window {
    pub shape: usize,
    pub w: usize,
    pub h: usize,
    cur_w: usize,
    cur_h: usize,
    pub state: WinState,
    /// Fixed cell contents (menus, labels, cursors) drawn over the fill.
    pub cells: HashMap<(usize, usize), Cell>,
    pub text: Option<TextRun>,
    grid: Option<Entity>,
    x: i32,
    y: i32,
}

impl Window {
    pub fn is_open(&self) -> bool {
        self.state == WinState::Open
    }
    pub fn put_text(&mut self, col: usize, row: usize, text: &str) {
        for (i, ch) in text.chars().enumerate() {
            self.cells.insert((col + i, row), Cell::byte(glyph_byte(ch)));
        }
    }
    pub fn put(&mut self, col: usize, row: usize, cell: Cell) {
        self.cells.insert((col, row), cell);
    }
    pub fn clear_cell(&mut self, col: usize, row: usize) {
        self.cells.remove(&(col, row));
    }
    pub fn close(&mut self) {
        if self.state != WinState::Closed {
            self.state = WinState::Closing;
        }
    }
    pub fn run_phase(&self) -> Option<RunPhase> {
        self.text.as_ref().map(|t| t.phase)
    }
}

/// All windows on screen.
#[derive(Resource, Default)]
pub struct Windows {
    map: HashMap<WindowId, Window>,
    next: u32,
    shapes: Vec<WindowShape>,
}

impl Windows {
    /// Opens a window of a cartridge shape; returns its id.
    pub fn open(&mut self, shape: usize) -> WindowId {
        let s = self.shapes.get(shape).copied().unwrap_or(WindowShape { w: 26, h: 10, x: 56, y: 24 });
        self.next += 1;
        let id = WindowId(self.next);
        self.map.insert(
            id,
            Window {
                shape,
                w: s.w,
                h: s.h,
                cur_w: 0,
                cur_h: 2,
                state: WinState::Opening,
                cells: HashMap::new(),
                text: None,
                grid: None,
                x: s.x,
                y: s.y,
            },
        );
        id
    }
    /// Opens a window that types dialogue `text_id` with the standard settings for its shape.
    pub fn open_text(&mut self, shape: usize, text_id: u32) -> WindowId {
        let id = self.open(shape);
        let w = self.map.get_mut(&id).unwrap();
        w.text = Some(TextRun::for_shape(text_id, shape, w.w, w.h));
        id
    }
    pub fn get(&self, id: WindowId) -> Option<&Window> {
        self.map.get(&id)
    }
    pub fn get_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.map.get_mut(&id)
    }
    /// True when the window is gone (closed or never existed).
    pub fn is_gone(&self, id: WindowId) -> bool {
        self.map.get(&id).is_none_or(|w| w.state == WinState::Closed)
    }
    pub fn close(&mut self, id: WindowId) {
        if let Some(w) = self.map.get_mut(&id) {
            w.close();
        }
    }
    pub fn any_open(&self) -> bool {
        self.map.values().any(|w| w.state != WinState::Closed)
    }
}

// ---------------------------------------------------------------------------
// Conversation window (NPCs, pickups, scripted messages)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Opening,
    Printing,
    WaitPage,
    WaitEnd,
    Halted,
    Closing,
}

/// Request to show a text: dialogue id (1-based) in a window shape.
#[derive(Debug, Clone)]
pub struct DialogRequest {
    pub id: u32,
    pub shape: usize,
    /// Player index whose name `{SPEAKER}` inserts.
    pub speaker: usize,
    /// Fixed lines for menu windows (id 0).
    pub lines: Option<Vec<String>>,
    /// Auto-advance pages/endings after these ticks (ending room).
    pub timed: Option<(u32, u32)>,
}

/// The conversation window. Gameplay pauses while it is open.
#[derive(Resource, Default)]
pub struct Dialog {
    pub window: Option<WindowId>,
    pub queue: Vec<DialogRequest>,
    /// Mirrors of the window's state, updated every tick.
    phase: Option<Phase>,
    shape: Option<usize>,
    replace: Option<u32>,
    close_requested: bool,
}

impl Dialog {
    pub fn open(&mut self, id: u32, shape: usize) {
        self.open_for(id, shape, 0);
    }
    /// Opens a text whose `{NAME}` inserts (F5) name player `speaker` (e.g. who found an item).
    pub fn open_for(&mut self, id: u32, shape: usize, speaker: usize) {
        self.queue.push(DialogRequest { id, shape, speaker, lines: None, timed: None });
    }
    /// Opens a text that pages and closes on timers instead of B (the ending room, 0x2F).
    pub fn open_timed(&mut self, id: u32, shape: usize, page: u32, end: u32) {
        self.queue.push(DialogRequest { id, shape, speaker: 0, lines: None, timed: Some((page, end)) });
    }
    pub fn is_open(&self) -> bool {
        self.window.is_some() || !self.queue.is_empty()
    }
    pub fn phase(&self) -> Option<Phase> {
        self.phase
    }
    pub fn active_shape(&self) -> Option<usize> {
        self.shape
    }
    /// Opens a window showing fixed lines immediately (menus); it stays open until closed.
    pub fn open_static(&mut self, shape: usize, lines: Vec<String>) {
        self.queue.push(DialogRequest { id: 0, shape, speaker: 0, lines: Some(lines), timed: None });
    }
    /// Closes a halted (YES/NO) or static window.
    pub fn close(&mut self) {
        self.close_requested = true;
    }
    /// Replaces the text of the open window (e.g. character-select descriptions).
    pub fn replace_text(&mut self, id: u32) {
        self.replace = Some(id);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_windows(
    mut commands: Commands,
    mut windows: ResMut<Windows>,
    mut dialog: ResMut<Dialog>,
    ui: Res<UiAssets>,
    defs: Res<Assets<DialogueDef>>,
    mut flags: ResMut<StoryFlags>,
    names: Res<PartyNames>,
    pads: Res<super::input::Joypads>,
    root: Query<Entity, With<ScreenRoot>>,
    mut sound: MessageWriter<super::audio::PlaySound>,
) {
    // Pages and endings wait for a new B press of either player (0xFF0211 / 0xFF0215).
    let pad = pads.any();
    let Some(def) = defs.get(&ui.dialogue) else { return };
    if windows.shapes.is_empty() {
        windows.shapes = def.windows.clone();
    }
    // Conversation window bookkeeping.
    if dialog.window.is_none() && !dialog.queue.is_empty() {
        let req = dialog.queue.remove(0);
        let id = if let Some(lines) = req.lines {
            let id = windows.open(req.shape);
            let w = windows.get_mut(id).unwrap();
            for (r, line) in lines.iter().enumerate() {
                w.put_text(2, 1 + r, line);
            }
            id
        } else {
            let id = windows.open_text(req.shape, req.id);
            if let Some(t) = windows.get_mut(id).and_then(|w| w.text.as_mut()) {
                t.speaker = req.speaker;
                if let Some((page, end)) = req.timed {
                    t.auto_page = Some(page);
                    t.auto_end = Some(end);
                    t.interval = 2;
                }
            }
            id
        };
        dialog.window = Some(id);
    }
    if let Some(id) = dialog.window {
        if let Some(w) = windows.get_mut(id) {
            if let Some(n) = dialog.replace.take() {
                if let Some(t) = w.text.as_mut() {
                    t.restart(n);
                }
            }
            if std::mem::take(&mut dialog.close_requested) {
                w.close();
            }
        }
    }

    let Ok(root) = root.single() else { return };
    for (id, w) in windows.map.iter_mut() {
        if w.grid.is_none() && w.state != WinState::Closed {
            // Later windows draw over earlier ones (all below the status bar).
            let depth = super::coords::z::HUD - 2.0 + (id.0 % 100) as f32 * 0.01;
            w.grid = Some(spawn_grid(&mut commands, root, w.x, w.y, w.w, w.h, depth));
        }
        match w.state {
            WinState::Opening => {
                if w.cur_w < w.w {
                    w.cur_w = (w.cur_w + 2).min(w.w);
                } else if w.cur_h < w.h {
                    w.cur_h = (w.cur_h + 2).min(w.h);
                }
                if w.cur_w >= w.w && w.cur_h >= w.h {
                    w.state = WinState::Open;
                }
            }
            WinState::Open => {
                if let Some(t) = w.text.as_mut() {
                    if t.tick(def, &mut flags, &names, &pad) {
                        sound.write(super::audio::PlaySound(0xB2));
                    }
                }
            }
            WinState::Closing => {
                if w.cur_h > 2 {
                    w.cur_h = w.cur_h.saturating_sub(2).max(2);
                } else if w.cur_w > 0 {
                    w.cur_w = w.cur_w.saturating_sub(2);
                } else {
                    w.state = WinState::Closed;
                    if let Some(g) = w.grid.take() {
                        commands.entity(g).despawn();
                    }
                }
            }
            WinState::Closed => {}
        }
    }
    // The conversation window closes itself when its text is done.
    if let Some(id) = dialog.window {
        let w = windows.get_mut(id);
        let (phase, shape) = match w {
            Some(w) => {
                if w.state == WinState::Open && w.run_phase() == Some(RunPhase::Done) {
                    w.close();
                }
                let p = match (w.state, w.run_phase()) {
                    (WinState::Opening, _) => Phase::Opening,
                    (WinState::Closing | WinState::Closed, _) => Phase::Closing,
                    (WinState::Open, None) | (WinState::Open, Some(RunPhase::Halted)) => Phase::Halted,
                    (WinState::Open, Some(RunPhase::Printing)) => Phase::Printing,
                    (WinState::Open, Some(RunPhase::WaitPage)) => Phase::WaitPage,
                    (WinState::Open, Some(RunPhase::WaitEnd | RunPhase::Done)) => Phase::WaitEnd,
                };
                (Some(p), Some(w.shape))
            }
            None => (None, None),
        };
        dialog.phase = phase;
        dialog.shape = shape;
        if windows.is_gone(id) {
            dialog.window = None;
            dialog.phase = None;
            dialog.shape = None;
        }
    }
    windows.map.retain(|_, w| w.state != WinState::Closed || w.grid.is_some());
}

/// Draws every window's frame, fill, fixed cells and text.
fn draw_windows(
    windows: Res<Windows>,
    ui: Res<UiAssets>,
    fonts: Res<Assets<FontDef>>,
    grids: Query<&CellGrid>,
    mut sprites: Query<(&mut Sprite, &mut Visibility)>,
) {
    let Some(font) = fonts.get(&ui.font) else { return };
    for w in windows.map.values() {
        let Some(grid_e) = w.grid else { continue };
        let Ok(grid) = grids.get(grid_e) else { continue };
        // Visible rectangle grows from the centre.
        let x0 = (w.w - w.cur_w) / 2;
        let y0 = (w.h - w.cur_h) / 2;
        let (x1, y1) = (x0 + w.cur_w, y0 + w.cur_h);
        let full = w.state == WinState::Open;
        for r in 0..w.h {
            for c in 0..w.w {
                let Ok((mut sprite, mut vis)) = sprites.get_mut(grid.cells[r * grid.cols + c]) else { continue };
                if c < x0 || c >= x1 || r < y0 || r >= y1 || w.cur_w == 0 || w.state == WinState::Closed {
                    *vis = Visibility::Hidden;
                    continue;
                }
                *vis = Visibility::Inherited;
                let (left, right, top, bottom) = (c == x0, c + 1 == x1, r == y0, r + 1 == y1);
                let frame = match (left, right, top, bottom) {
                    (true, _, true, _) => Some(Cell { glyph: "0x5D".into(), hflip: false, vflip: false }),
                    (_, true, true, _) => Some(Cell { glyph: "0x5D".into(), hflip: true, vflip: false }),
                    (true, _, _, true) => Some(Cell { glyph: "0x5D".into(), hflip: false, vflip: true }),
                    (_, true, _, true) => Some(Cell { glyph: "0x5D".into(), hflip: true, vflip: true }),
                    (_, _, true, _) => Some(Cell { glyph: "0x5E".into(), hflip: false, vflip: false }),
                    (_, _, _, true) => Some(Cell { glyph: "0x5E".into(), hflip: false, vflip: true }),
                    (true, _, _, _) => Some(Cell { glyph: "0x5F".into(), hflip: false, vflip: false }),
                    (_, true, _, _) => Some(Cell { glyph: "0x5F".into(), hflip: true, vflip: false }),
                    _ => None,
                };
                // Fixed cells (also on the border, e.g. names) override; text fills the interior.
                let cell = if full { w.cells.get(&(c, r)).cloned() } else { None }
                    .or(frame)
                    .or_else(|| {
                        let t = w.text.as_ref().filter(|_| full)?;
                        let (tc, tr) = (c.checked_sub(t.origin.0)?, r.checked_sub(t.origin.1)?);
                        t.cell(tc, tr)
                    })
                    .unwrap_or_else(Cell::blank);
                super::ui::set_cell(font, &mut sprite, &cell);
            }
        }
    }
}

pub fn dialog_closed(dialog: Res<Dialog>, windows: Res<Windows>) -> bool {
    !dialog.is_open() && !windows.any_open()
}

pub fn plugin(app: &mut App) {
    app.init_resource::<Dialog>()
        .init_resource::<Windows>()
        .init_resource::<StoryFlags>()
        .init_resource::<PartyNames>()
        .add_systems(FixedUpdate, run_windows.in_set(GameSystems::Game).before(super::player::tick))
        .add_systems(Update, draw_windows);
}
