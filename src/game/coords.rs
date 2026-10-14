//! World coordinates follow the original game: x grows right, y grows down, units are pixels
//! with 16 fractional bits (16.16 fixed point, like the cartridge's word + sub-pixel word).
//! Bevy transforms are derived from these each frame.
use bevy::prelude::*;

pub const ONE: i32 = 1 << 16;

/// Converts a pixel amount (possibly fractional) into 16.16 fixed point.
pub const fn px(v: i32) -> i32 {
    v << 16
}

/// Position of an actor's anchor point (usually its feet) in world space, 16.16 fixed point.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorldPos {
    pub x: i32,
    pub y: i32,
}

impl WorldPos {
    pub fn from_px(x: i32, y: i32) -> Self {
        Self { x: px(x), y: px(y) }
    }
    pub fn px_x(&self) -> i32 {
        self.x >> 16
    }
    pub fn px_y(&self) -> i32 {
        self.y >> 16
    }
}

/// Velocity in 16.16 fixed point pixels per tick.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Velocity {
    pub x: i32,
    pub y: i32,
}

/// Draw order layers (Bevy z).
pub mod z {
    pub const BACKGROUND: f32 = 0.0;
    pub const FOREGROUND: f32 = 10.0;
    pub const ACTORS: f32 = 20.0;
    pub const PLAYER: f32 = 25.0;
    pub const FOREGROUND_HIGH: f32 = 30.0;
    pub const HUD: f32 = 40.0;
}

/// Converts a world pixel position (y down) into Bevy space (y up).
pub fn to_bevy(x: i32, y: i32, z: f32) -> Vec3 {
    Vec3::new(x as f32, -(y as f32), z)
}
