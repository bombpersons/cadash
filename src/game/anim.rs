//! Frame-based sprite animation driven by extracted sprite sheets.
//!
//! Timing mirrors the cartridge's animation stepper: each frame has a duration in ticks,
//! after the last frame playback jumps to the animation's loop index and a "cycle finished"
//! flag is raised that gameplay code can react to.
use super::{
    coords::{WorldPos, to_bevy},
    data::{ACTOR_DURATION, SpriteSheet},
};
use crate::GameSystems;
use bevy::{prelude::*, sprite::Anchor};

#[derive(Component, Debug, Clone)]
pub struct SpriteAnimator {
    pub sheet: Handle<SpriteSheet>,
    pub anim: String,
    pub index: usize,
    pub timer: u32,
    /// Duration used by frames flagged with [`ACTOR_DURATION`] (e.g. attack speed).
    pub actor_duration: u16,
    /// Set when playback wrapped past the last frame at least once.
    pub cycled: bool,
    /// Draw depth.
    pub z: f32,
    /// Stepped by its owner instead of the generic animation system.
    pub manual: bool,
    /// A newly started animation shows its first frame for the full duration (objects) instead
    /// of duration-1 ticks (player, whose animation is stepped in the same tick it is set).
    pub first_frame_full: bool,
    started: bool,
}

impl SpriteAnimator {
    pub fn new(sheet: Handle<SpriteSheet>, anim: impl Into<String>, z: f32) -> Self {
        Self {
            sheet,
            anim: anim.into(),
            index: 0,
            timer: 0,
            actor_duration: 1,
            cycled: false,
            z,
            manual: false,
            first_frame_full: false,
            started: false,
        }
    }

    /// Switches animation and always restarts from its first frame.
    pub fn restart(&mut self, anim: &str) {
        self.anim = anim.to_owned();
        self.index = 0;
        self.cycled = false;
        self.started = false;
    }

    /// Changes animation keeping the current frame position (used for facing changes).
    pub fn swap_keep_frame(&mut self, anim: &str) {
        self.anim = anim.to_owned();
    }
}

fn frame_duration(dur: u16, actor: u16) -> u32 {
    if dur == ACTOR_DURATION { actor.max(1) as u32 } else { dur.max(1) as u32 }
}

/// Advances one animation by one tick.
pub fn step(sheet: &SpriteSheet, a: &mut SpriteAnimator) {
    let Some(def) = sheet.anims.get(&a.anim) else { return };
    if def.frames.is_empty() {
        return;
    }
    if !a.started {
        a.started = true;
        a.index = a.index.min(def.frames.len() - 1);
        a.timer = frame_duration(def.frames[a.index].dur, a.actor_duration);
        if a.first_frame_full {
            return;
        }
    }
    a.timer = a.timer.saturating_sub(1);
    if a.timer == 0 {
        a.index += 1;
        if a.index >= def.frames.len() {
            a.index = def.loop_to.min(def.frames.len() - 1);
            a.cycled = true;
        }
        a.timer = frame_duration(def.frames[a.index].dur, a.actor_duration);
    }
}

pub fn advance(sheets: Res<Assets<SpriteSheet>>, mut q: Query<&mut SpriteAnimator>) {
    for mut a in &mut q {
        if a.manual {
            continue;
        }
        let Some(sheet) = sheets.get(&a.sheet) else { continue };
        // The cartridge sets the timer and decrements it in the same tick, so a freshly
        // started player animation shows its first frame for duration-1 ticks.
        step(sheet, &mut a);
    }
}

/// Places every animated sprite: selects the atlas rect and offsets it by the frame origin.
pub fn sync_sprites(
    sheets: Res<Assets<SpriteSheet>>,
    mut q: Query<(&SpriteAnimator, &WorldPos, &mut Sprite, &mut Transform)>,
) {
    for (a, pos, mut sprite, mut tf) in &mut q {
        let Some(sheet) = sheets.get(&a.sheet) else { continue };
        let Some(def) = sheet.anims.get(&a.anim) else { continue };
        let Some(af) = def.frames.get(a.index) else { continue };
        let f = &sheet.frames[af.frame];
        if sprite.image != sheet.image {
            sprite.image = sheet.image.clone();
        }
        let [x, y, w, h] = f.rect;
        sprite.rect = Some(Rect::new(x as f32, y as f32, (x + w) as f32, (y + h) as f32));
        tf.translation = to_bevy(pos.px_x() - f.origin[0], pos.px_y() - f.origin[1], a.z);
    }
}

pub fn plugin(app: &mut App) {
    app.add_systems(FixedUpdate, advance.in_set(GameSystems::Animate).run_if(super::room_events::world_running))
        .add_systems(PostUpdate, sync_sprites.before(TransformSystems::Propagate));
}

/// Bundle helper for an animated actor sprite.
pub fn animated_sprite(animator: SpriteAnimator) -> impl Bundle {
    (animator, Sprite::default(), Anchor::TOP_LEFT)
}
