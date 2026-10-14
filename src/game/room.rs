//! Rooms: pre-rendered foreground layers (split by draw priority), a parallax background that
//! wraps at 512x256, palette-cycling overlays, and the block attribute grid used for collision.
use super::{
    camera::CameraPos,
    coords::{to_bevy, z},
    data::{PaletteCycle, RoomDef},
    ui::ScreenRoot,
};
use bevy::{prelude::*, sprite::Anchor};

/// The room currently being played.
#[derive(Resource, Debug, Clone)]
pub struct CurrentRoom {
    pub index: usize,
    pub def: Handle<RoomDef>,
}

/// Marks entities that belong to the active room and are removed when it changes.
#[derive(Component)]
pub struct RoomEntity;

/// Background layer: scrolls by `factor` of the camera movement since room entry.
#[derive(Component)]
pub struct BackgroundLayer {
    pub entry_camera: IVec2,
    pub factor: Vec2,
    /// Extra offset in background pixels (screen pixel x shows background pixel x + offset.x), used by
    /// scripted room events (`room_events`: plane-B scroll registers set by the cartridge, shakes).
    pub offset: IVec2,
    /// Plane-B scroll registers written by object scripts (Balrog's background-plane form,
    /// `bosses::render_plane_b`): screen pixel (x, y) shows background pixel (x - h, y + v).
    /// Reset with the layer on every room load, like the cartridge's B scroll.
    pub script_scroll: IVec2,
}

/// Marks the room's foreground sprites (both priorities and their palette-cycle masks) so scripted
/// room events can swap their image or shake them.
#[derive(Component)]
pub struct ForegroundLayer {
    pub high: bool,
}

/// One tile of the wrapped background (2x2 copies cover the screen).
#[derive(Component)]
pub struct BackgroundTile {
    pub ix: i32,
    pub iy: i32,
}

/// Animated colour for a palette-cycle mask.
#[derive(Component)]
pub struct CycleMask {
    pub cycle: usize,
}

/// Playback state for the room's palette cycles.
#[derive(Resource, Default)]
pub struct CycleState {
    pub steps: Vec<(usize, u32)>,
    /// Cycle set replacing the room's own (`RoomDef::palette_cycles`) after a scripted swap
    /// (cartridge 0x2C398 called with another table); cleared on room entry.
    pub override_cycles: Option<Vec<PaletteCycle>>,
}

pub fn room_path(index: usize) -> String {
    format!("rooms/room_{index:02}.room.json")
}

pub const BG_W: i32 = 512;
pub const BG_H: i32 = 256;

/// Spawns the layer sprites for a room. Call once its [`RoomDef`] is loaded.
pub fn spawn_room_layers(
    commands: &mut Commands,
    def: &RoomDef,
    screen_root: Entity,
    camera: &CameraPos,
    cycles: &mut CycleState,
) {
    let fg_layers = [(def.fg_image.clone(), z::FOREGROUND, false), (def.fg_hi_image.clone(), z::FOREGROUND_HIGH, true)];
    for (image, depth, high) in fg_layers {
        commands.spawn((
            Sprite::from_image(image),
            Anchor::TOP_LEFT,
            Transform::from_translation(to_bevy(0, 0, depth)),
            ForegroundLayer { high },
            RoomEntity,
        ));
    }
    // Background in screen space, under the camera.
    let bg = commands
        .spawn((
            BackgroundLayer {
                entry_camera: IVec2::new(camera.x, camera.y),
                factor: Vec2::from(def.bg_scroll),
                offset: IVec2::ZERO,
                script_scroll: IVec2::ZERO,
            },
            Transform::from_xyz(0.0, 0.0, z::BACKGROUND),
            Visibility::Inherited,
            RoomEntity,
            ChildOf(screen_root),
        ))
        .id();
    // Backdrop colour behind everything (palette line 2 colour 0).
    let [r, g, b] = def.backdrop;
    commands.spawn((
        Sprite::from_color(Color::srgb_u8(r, g, b), Vec2::new(320.0, 224.0)),
        Anchor::TOP_LEFT,
        Transform::from_xyz(0.0, 0.0, -1.0),
        ChildOf(bg),
    ));
    for iy in 0..2 {
        for ix in 0..2 {
            let tile = commands
                .spawn((
                    Sprite::from_image(def.bg_image.clone()),
                    Anchor::TOP_LEFT,
                    Transform::default(),
                    BackgroundTile { ix, iy },
                    ChildOf(bg),
                ))
                .id();
            for (cycle, masks) in def.cycle_images.iter().enumerate() {
                for (layer, image) in masks {
                    if layer == "bg" {
                        commands.spawn((
                            Sprite::from_image(image.clone()),
                            Anchor::TOP_LEFT,
                            Transform::from_xyz(0.0, 0.0, 0.1),
                            CycleMask { cycle },
                            ChildOf(tile),
                        ));
                    }
                }
            }
        }
    }
    for (cycle, masks) in def.cycle_images.iter().enumerate() {
        for (layer, image) in masks {
            if layer == "fg" {
                commands.spawn((
                    Sprite::from_image(image.clone()),
                    Anchor::TOP_LEFT,
                    Transform::from_translation(to_bevy(0, 0, z::FOREGROUND + 0.1)),
                    CycleMask { cycle },
                    ForegroundLayer { high: false },
                    RoomEntity,
                ));
            }
        }
    }
    cycles.steps = vec![(0, 0); def.palette_cycles.len()];
    cycles.override_cycles = None;
}

pub fn despawn_room(commands: &mut Commands, q: &Query<Entity, With<RoomEntity>>) {
    for e in q {
        commands.entity(e).despawn();
    }
}

/// Positions the wrapped background according to camera movement since room entry.
pub fn scroll_background(
    cam: Res<CameraPos>,
    layers: Query<(&BackgroundLayer, &Children)>,
    mut tiles: Query<(&BackgroundTile, &mut Transform)>,
) {
    for (layer, children) in &layers {
        let dx = ((cam.x - layer.entry_camera.x) as f32 * layer.factor.x).floor() as i32 + layer.offset.x - layer.script_scroll.x;
        let dy = ((cam.y - layer.entry_camera.y) as f32 * layer.factor.y).floor() as i32 + layer.offset.y + layer.script_scroll.y;
        let ox = -dx.rem_euclid(BG_W);
        let oy = -dy.rem_euclid(BG_H);
        for child in children.iter() {
            if let Ok((t, mut tf)) = tiles.get_mut(child) {
                tf.translation.x = (ox + t.ix * BG_W) as f32;
                tf.translation.y = -((oy + t.iy * BG_H) as f32);
            }
        }
    }
}

/// Steps palette cycles (cartridge table 0x30B04) and tints their masks.
pub fn animate_cycles(
    room: Option<Res<CurrentRoom>>,
    rooms: Res<Assets<RoomDef>>,
    mut state: ResMut<CycleState>,
    mut masks: Query<(&CycleMask, &mut Sprite)>,
) {
    let Some(def) = room.and_then(|r| rooms.get(&r.def)) else { return };
    let CycleState { steps, override_cycles } = &mut *state;
    let cycles: &[PaletteCycle] = override_cycles.as_deref().unwrap_or(&def.palette_cycles);
    for (i, cycle) in cycles.iter().enumerate() {
        let Some(st) = steps.get_mut(i) else { continue };
        if cycle.steps.is_empty() {
            continue;
        }
        st.1 += 1;
        if st.1 >= cycle.steps[st.0].frames.max(1) {
            st.1 = 0;
            st.0 = (st.0 + 1) % cycle.steps.len();
        }
    }
    for (m, mut sprite) in &mut masks {
        if let (Some(cycle), Some(st)) = (cycles.get(m.cycle), steps.get(m.cycle)) {
            let [r, g, b] = cycle.steps[st.0].rgb;
            sprite.color = Color::srgb_u8(r, g, b);
        }
    }
}

/// The screen-space root (child of the game camera) used for HUD and background.
pub fn screen_root(roots: &Query<Entity, With<ScreenRoot>>) -> Option<Entity> {
    roots.single().ok()
}

pub fn plugin(app: &mut App) {
    app.init_resource::<CycleState>()
        .add_systems(FixedUpdate, animate_cycles.in_set(crate::GameSystems::Animate))
        .add_systems(PostUpdate, scroll_background.before(TransformSystems::Propagate));
}
