//! Gameplay joypads: the cartridge only accepts a button once it has read the same value on two
//! consecutive frames, so every press reaches the game one tick late.
//!
//! [`Joypads`] holds one debounced pad per controller port (player slot); the [`Joypad`] resource
//! is a copy of pad 1 for menus and other single-player code paths.
use crate::{
    GameSystems,
    controller::{Button, Controllers, PORTS},
};
use bevy::prelude::*;

#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct Joypad {
    raw_prev: u16,
    held: u16,
    prev_held: u16,
}

impl Joypad {
    pub fn held(&self, b: Button) -> bool {
        self.held & b.mask() != 0
    }
    pub fn pressed(&self, b: Button) -> bool {
        (self.held & !self.prev_held) & b.mask() != 0
    }
    pub fn released(&self, b: Button) -> bool {
        (!self.held & self.prev_held) & b.mask() != 0
    }
    /// Feeds one raw read through the two-read debounce.
    fn feed(&mut self, raw: u16) {
        self.prev_held = self.held;
        let stable = !(raw ^ self.raw_prev);
        self.held = (self.held & !stable) | (raw & stable);
        self.raw_prev = raw;
    }
    /// Combination of two pads (a button counts if either pad has it).
    pub fn merged(&self, o: &Joypad) -> Joypad {
        Joypad { raw_prev: self.raw_prev | o.raw_prev, held: self.held | o.held, prev_held: self.prev_held | o.prev_held }
    }
}

/// Debounced pads per controller port; player slot `n` reads port `n`.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct Joypads {
    pub ports: [Joypad; PORTS],
}

impl Joypads {
    /// Pad of player slot `slot` (an idle pad for slots without a port).
    pub fn get(&self, slot: usize) -> Joypad {
        self.ports.get(slot).copied().unwrap_or_default()
    }
    /// Either pad (e.g. dialogue pages advance on a B press by any player).
    pub fn any(&self) -> Joypad {
        self.ports.iter().fold(Joypad::default(), |a, p| a.merged(p))
    }
}

/// Recorded pad input that replaces the controllers (attract demo, ending room; cartridge 0xC06).
/// Streams are {ticks, buttons} pairs with cartridge button bits (7 Start, 6 A, 5 C, 4 B, 3 R, 2 L, 1 D, 0 U),
/// one per controller port. The replayed bits go through the same two-read debounce as a real pad.
#[derive(Resource, Default, Debug)]
pub struct DemoInput {
    ports: Vec<DemoStream>,
    active: bool,
    finished: bool,
}

#[derive(Default, Debug, Clone)]
struct DemoStream {
    stream: Vec<[u8; 2]>,
    index: usize,
    left: u32,
}

impl DemoStream {
    /// Cartridge bits for this tick, or None once the stream is exhausted (0x0000 terminator).
    fn next(&mut self) -> Option<u8> {
        if self.left == 0 {
            let &[n, _] = self.stream.get(self.index)?;
            self.left = n as u32;
            self.index += 1;
        }
        self.left = self.left.saturating_sub(1);
        Some(self.stream[self.index - 1][1])
    }
}

impl DemoInput {
    /// Replays one stream on every port (the ending room feeds 0x632A to both pads, 0x3100E).
    pub fn start(&mut self, stream: Vec<[u8; 2]>) {
        self.start_ports(vec![stream; PORTS], 0);
    }
    /// Replays one stream per port (port 0 = pad 1), dropping the first `skip` ticks of each.
    pub fn start_ports(&mut self, streams: Vec<Vec<[u8; 2]>>, skip: u32) {
        let ports = streams.into_iter().map(|stream| DemoStream { stream, index: 0, left: 0 }).collect();
        *self = Self { ports, active: true, finished: false };
        for _ in 0..skip {
            self.next_raw();
        }
    }
    pub fn stop(&mut self) {
        self.active = false;
    }
    pub fn active(&self) -> bool {
        self.active
    }
    /// A stream ran out (cartridge 0xFF0A72 = 3); the first port to run out ends the replay.
    pub fn finished(&self) -> bool {
        self.finished
    }
    fn next_raw(&mut self) -> Option<Vec<u8>> {
        if !self.active {
            return None;
        }
        let mut out = Vec::with_capacity(self.ports.len());
        for p in &mut self.ports {
            match p.next() {
                Some(b) => out.push(b),
                None => {
                    self.finished = true;
                    out.push(0);
                }
            }
        }
        Some(out)
    }
    /// Buttons for this tick per port as [`Button`] masks, or None when not replaying.
    fn next(&mut self) -> Option<Vec<u16>> {
        let map = [Button::Up, Button::Down, Button::Left, Button::Right, Button::B, Button::C, Button::A, Button::Start];
        let raw = self.next_raw()?;
        Some(
            raw.into_iter()
                .map(|bits| map.iter().enumerate().filter(|(i, _)| bits & (1 << i) != 0).fold(0u16, |m, (_, b)| m | b.mask()))
                .collect(),
        )
    }
}

pub fn sample(pads: Res<Controllers>, mut joy: ResMut<Joypad>, mut joys: ResMut<Joypads>, mut demo: ResMut<DemoInput>) {
    let demo_bits = demo.next();
    for (port, pad) in pads.ports.iter().enumerate() {
        let live = Button::ALL.into_iter().filter(|b| pad.held(*b)).fold(0u16, |acc, b| acc | b.mask());
        // A replay drives every port that has a stream; the others read as idle.
        let raw = match &demo_bits {
            Some(bits) => bits.get(port).copied().unwrap_or(0),
            None => live,
        };
        joys.ports[port].feed(raw);
    }
    *joy = joys.ports[0];
}

pub fn plugin(app: &mut App) {
    app.init_resource::<Joypad>().init_resource::<Joypads>().init_resource::<DemoInput>().add_systems(
        FixedUpdate,
        sample.in_set(GameSystems::Input).after(crate::begin_tick).run_if(|c: Res<crate::GameClock>| c.ticking),
    );
}
