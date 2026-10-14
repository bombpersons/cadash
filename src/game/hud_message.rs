//! Messages typed into the status bar (cartridge 0x8C9A): one glyph per tick into a 17x2 area,
//! pages and endings wait 60 ticks, then the normal status bar returns. Every player has their own
//! message line (queue at player +0xB2/+0xB4) in their half of the status bar.
use super::{
    data::{DialogueDef, Token},
    dialog::PartyNames,
    lifecycle::Lives,
    ui::{UiAssets, glyph_byte},
};
use crate::GameSystems;
use bevy::prelude::*;

pub const COLS: usize = 17;
pub const ROWS: usize = 2;

/// Status-bar messages, one channel per player slot.
#[derive(Resource, Default, Debug)]
pub struct HudMessages {
    channels: Vec<Channel>,
}

#[derive(Default, Debug)]
struct Channel {
    queue: Vec<u32>,
    active: Option<Active>,
}

#[derive(Debug)]
struct Active {
    node: Option<String>,
    token: usize,
    pending: Vec<u8>,
    col: usize,
    row: usize,
    wait: u32,
    finish_after_wait: bool,
    /// Reached the end code: the text stays up (dead players) until replaced or dismissed.
    holding: bool,
    pub cells: [[u8; COLS]; ROWS],
}

impl HudMessages {
    /// Queues HUD message `id` (dialogue entry 138 + id) on player 1's line.
    #[allow(dead_code)]
    pub fn push(&mut self, id: u32) {
        self.push_to(0, id);
    }
    /// Queues HUD message `id` on the status line of player `slot`.
    pub fn push_to(&mut self, slot: usize, id: u32) {
        if self.channels.len() <= slot {
            self.channels.resize_with(slot + 1, Channel::default);
        }
        self.channels[slot].queue.push(id);
    }
    /// Text cells currently shown on player `slot`'s line, if a message is active.
    pub fn cells_for(&self, slot: usize) -> Option<&[[u8; COLS]; ROWS]> {
        self.channels.get(slot)?.active.as_ref().map(|a| &a.cells)
    }
    /// True while a message is being typed/shown on `slot`'s line (cartridge player +0xB2 != 0).
    /// A message that has reached its end code and only stays on screen does not count.
    pub fn busy(&self, slot: usize) -> bool {
        self.channels.get(slot).is_some_and(|c| c.active.as_ref().is_some_and(|a| !a.holding) || !c.queue.is_empty())
    }
    /// True when `slot`'s line shows a finished message that is only kept on screen.
    pub fn holding(&self, slot: usize) -> bool {
        self.channels.get(slot).is_some_and(|c| c.active.as_ref().is_some_and(|a| a.holding))
    }
    /// Removes a finished message from `slot`'s line (the status bar is redrawn).
    pub fn dismiss(&mut self, slot: usize) {
        if let Some(c) = self.channels.get_mut(slot) {
            if c.active.as_ref().is_some_and(|a| a.holding) {
                c.active = None;
            }
        }
    }
    /// Drops every message (new game).
    pub fn clear(&mut self) {
        self.channels.clear();
    }
}

fn run(
    mut msgs: ResMut<HudMessages>,
    ui: Res<UiAssets>,
    defs: Res<Assets<DialogueDef>>,
    names: Res<PartyNames>,
    lives: Res<Lives>,
) {
    let Some(def) = defs.get(&ui.dialogue) else { return };
    for (slot, ch) in msgs.channels.iter_mut().enumerate() {
        let name = if slot == 0 { names.p1.clone() } else { names.p2.clone().unwrap_or_default() };
        run_channel(ch, def, &name, &names.p1, lives.0);
    }
}

fn run_channel(msgs: &mut Channel, def: &DialogueDef, name: &str, p1: &str, lives: u32) {
    // A finished message only stays on screen until the next one arrives.
    if msgs.active.as_ref().is_some_and(|a| a.holding) && !msgs.queue.is_empty() {
        msgs.active = None;
    }
    if msgs.active.is_none() {
        if msgs.queue.is_empty() {
            return;
        }
        let id = msgs.queue.remove(0);
        msgs.active = Some(Active {
            node: def.entries.get(&(138 + id).to_string()).cloned(),
            token: 0,
            pending: vec![],
            col: 0,
            row: 0,
            wait: 0,
            finish_after_wait: false,
            holding: false,
            cells: [[0x3A; COLS]; ROWS],
        });
    }
    let a = msgs.active.as_mut().unwrap();
    if a.wait > 0 {
        a.wait -= 1;
        if a.wait == 0 {
            if a.finish_after_wait {
                msgs.active = None;
            } else {
                a.cells = [[0x3A; COLS]; ROWS];
                a.col = 0;
                a.row = 0;
            }
        }
        return;
    }
    // Print one glyph per tick.
    loop {
        if let Some(g) = (!a.pending.is_empty()).then(|| a.pending.remove(0)) {
            if a.row < ROWS && a.col < COLS {
                a.cells[a.row][a.col] = g;
            }
            a.col += 1;
            return;
        }
        let Some(addr) = a.node.clone() else {
            a.wait = 60;
            a.finish_after_wait = true;
            return;
        };
        let Some(node) = def.nodes.get(&addr) else {
            a.node = None;
            continue;
        };
        let Some(tok) = node.tokens.get(a.token).cloned() else {
            a.node = node.next.clone();
            a.token = 0;
            if a.node.is_none() {
                a.wait = 60;
                a.finish_after_wait = true;
                return;
            }
            continue;
        };
        a.token += 1;
        match tok {
            Token::Text { s } => a.pending.extend(s.chars().map(glyph_byte)),
            Token::PlayerName | Token::SpeakerName => a.pending.extend(name.chars().map(glyph_byte)),
            Token::P1Name => a.pending.extend(p1.chars().map(glyph_byte)),
            Token::Nl => {
                a.row += 1;
                a.col = 0;
            }
            Token::Page => {
                a.wait = 60;
                return;
            }
            Token::Cont => {
                a.wait = 60;
                a.finish_after_wait = true;
                return;
            }
            Token::End => {
                // The message stays until something replaces it.
                a.node = None;
                a.wait = u32::MAX;
                a.holding = true;
                return;
            }
            Token::Goto { target } => {
                a.node = Some(target);
                a.token = 0;
            }
            Token::IfNoLives { target } => {
                if lives == 0 {
                    a.node = Some(target);
                    a.token = 0;
                }
            }
            _ => {}
        }
    }
}

pub fn plugin(app: &mut App) {
    app.init_resource::<HudMessages>().add_systems(FixedUpdate, run.in_set(GameSystems::Game));
}
