//! Normal Bevy game entities render on layer0 into this fixed-resolution target.
use bevy::{
    camera::{RenderTarget, visibility::RenderLayers},
    prelude::*,
    render::render_resource::TextureFormat,
};

#[derive(Resource)]
pub struct Screen {
    pub width: u32,
    pub height: u32,
    pub image: Handle<Image>,
}
impl Default for Screen {
    fn default() -> Self {
        Self {
            width: 320,
            height: 224,
            image: default(),
        }
    }
}

#[derive(Component)]
pub struct DisplayQuad;

/// The camera that renders the game world into the fixed-resolution target.
#[derive(Component)]
pub struct GameCamera;

pub fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut screen: ResMut<Screen>,
) {
    let mut image = Image::new_target_texture(
        screen.width,
        screen.height,
        TextureFormat::Rgba8UnormSrgb,
        None,
    );
    image.texture_descriptor.usage |= bevy::render::render_resource::TextureUsages::COPY_SRC;
    screen.image = images.add(image);
    commands.spawn((
        Camera2d,
        bevy::render::view::Msaa::Off,
        Camera {
            order: -1,
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        RenderTarget::Image(screen.image.clone().into()),
        RenderLayers::layer(0),
        GameCamera,
    ));
    commands.spawn((
        Camera2d,
        bevy::render::view::Msaa::Off,
        Camera {
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            ..default()
        },
        RenderLayers::layer(1),
    ));
    commands.spawn((
        Sprite::from_image(screen.image.clone()),
        Transform::default(),
        DisplayQuad,
        RenderLayers::layer(1),
    ));
}

pub fn fit_scale(window: Vec2, screen: Vec2) -> f32 {
    (window.x / screen.x).min(window.y / screen.y).max(0.0)
}

pub fn fit_window(
    windows: Query<&Window>,
    screen: Res<Screen>,
    mut display: Query<&mut Transform, With<DisplayQuad>>,
) {
    let Ok(window) = windows.single() else {
        return;
    };
    let scale = fit_scale(
        Vec2::new(window.width(), window.height()),
        Vec2::new(screen.width as f32, screen.height as f32),
    );
    for mut transform in &mut display {
        transform.scale = Vec3::splat(scale);
    }
}

/// Makes the window visible a few frames after start-up (it is created hidden).
pub fn show_window(mut windows: Query<&mut Window>, mut frames: Local<u32>) {
    if *frames > 3 {
        return;
    }
    *frames += 1;
    if *frames == 3 {
        for mut window in &mut windows {
            window.visible = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wide_tall_and_small_windows_fit_without_stretching() {
        let screen = Vec2::new(320., 224.);
        for window in [
            Vec2::new(1600., 900.),
            Vec2::new(300., 900.),
            Vec2::new(160., 112.),
        ] {
            let fitted = screen * fit_scale(window, screen);
            assert!(fitted.x <= window.x + 0.001 && fitted.y <= window.y + 0.001);
            assert!((fitted.x / fitted.y - 320. / 224.).abs() < 0.0001);
        }
    }
}
