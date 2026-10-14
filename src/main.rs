//! Native game shell: fixed-resolution rendering, fixed-step logic and controllers.
mod controller;
mod game;
#[cfg(feature = "dev-mcp")]
mod dev_mcp;
mod screen;

use bevy::prelude::*;
use bevy::render::{RenderPlugin, settings::WgpuSettings};
use controller::{ControllerBindings, Controllers};

#[derive(Resource, Default)]
pub struct GameClock {
    pub frame: u64,
    /// Game tick counter (cartridge 0xFF0240, +1 per main-loop pass): logic that runs on odd/even
    /// ticks reads this, not `frame`. The attract demo sets it to the cartridge's value.
    pub tick: u64,
    pub ticking: bool,
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum GameSystems {
    Input,
    /// Gameplay logic (input handling, AI, physics).
    Game,
    /// Animation playback after gameplay decided what to play.
    Animate,
    /// Camera follows final positions.
    Camera,
    Finish,
}

fn main() {
    let mcp = std::env::args().any(|arg| arg == "--mcp");
    let software = std::env::args().any(|arg| arg == "--software");
    #[cfg(not(feature = "dev-mcp"))]
    if mcp {
        eprintln!("--mcp requires a developer build: cargo run --features dev-mcp -- --mcp");
        std::process::exit(2);
    }
    // The assets are not distributed: they are extracted from the player's own cartridge image.
    let assets = bevy::asset::io::file::FileAssetReader::get_base_path().join("assets");
    if !assets.join("data/physics.data.json").exists() {
        eprintln!(
            "No game assets in {}.\nExtract them from the Cadash (USA, Korea) cartridge image first:\n  \
             python3 extract/extract_assets.py \"Cadash (USA, Korea) (En).md\" --mdx /path/to/mdx",
            assets.display()
        );
        std::process::exit(1);
    }
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title: "Cadash".into(),
                    resolution: (960, 672).into(),
                    // Shown by `screen::show_window` once the first frames are drawn at the right
                    // size, so no undersized frame flashes in the corner at start-up.
                    visible: false,
                    ..default()
                }),
                ..default()
            })
            .set(ImagePlugin::default_nearest())
            // Normal WGPU selection prefers hardware and permits CPU adapters when
            // hardware is absent. GLES adds another backend; --software selects a
            // software adapter explicitly (e.g. Mesa lavapipe/llvmpipe).
            .set(RenderPlugin {
                render_creation: WgpuSettings {
                    force_fallback_adapter: software,
                    ..default()
                }
                .into(),
                synchronous_pipeline_compilation: true,
                ..default()
            }),
    )
    .insert_resource(Time::<Fixed>::from_hz(60.0))
    .init_resource::<GameClock>()
    .init_resource::<ControllerBindings>()
    .init_resource::<Controllers>()
    .init_resource::<screen::Screen>()
    .configure_sets(
        FixedUpdate,
        (
            GameSystems::Input,
            GameSystems::Game,
            GameSystems::Animate,
            GameSystems::Camera,
            GameSystems::Finish,
        )
            .chain(),
    )
    .configure_sets(
        FixedUpdate,
        (GameSystems::Game, GameSystems::Animate, GameSystems::Camera)
            .run_if(|clock: Res<GameClock>| clock.ticking),
    )
    .add_plugins(game::CadashPlugin)
    .add_systems(Startup, screen::setup)
    .add_systems(Update, (screen::fit_window, screen::show_window))
    .add_systems(FixedUpdate, begin_tick.in_set(GameSystems::Input))
    .add_systems(FixedUpdate, finish_tick.in_set(GameSystems::Finish));
    #[cfg(feature = "dev-mcp")]
    if mcp {
        app.add_plugins(dev_mcp::GameMcpPlugin);
    }
    app.run();
}

pub(crate) fn begin_tick(
    keys: Res<ButtonInput<KeyCode>>,
    logical: Res<ButtonInput<bevy::input::keyboard::Key>>,
    pads: Query<(Entity, &Gamepad)>,
    bindings: Res<ControllerBindings>,
    mut state: ResMut<Controllers>,
    mut clock: ResMut<GameClock>,
    #[cfg(feature = "dev-mcp")] mut control: Option<ResMut<dev_mcp::Control>>,
) {
    clock.ticking = true;
    #[cfg(not(feature = "dev-mcp"))]
    let injected = [0u16; controller::PORTS];
    #[cfg(feature = "dev-mcp")]
    let mut injected = [0u16; controller::PORTS];
    #[cfg(feature = "dev-mcp")]
    if let Some(ref mut control) = control {
        control.discard_cancelled_step();
        clock.ticking = !control.paused || control.has_step();
        injected = [control.buttons, control.buttons2];
    }
    if clock.ticking {
        // Gamepads are assigned to ports in connection (entity) order.
        let mut gamepads: Vec<_> = pads.iter().collect();
        gamepads.sort_by_key(|(entity, _)| entity.to_bits());
        for port in 0..controller::PORTS {
            let pad = gamepads.get(port).map(|(_, pad)| *pad);
            let physical = controller::sample_port(port, &controller::Keys { codes: &keys, logical: Some(&logical) }, pad, &bindings);
            state.ports[port].update(physical | injected[port]);
            state.connected[port] = port == 0 || pad.is_some() || bindings.keyboard_p2_enabled;
        }
    }
}

fn finish_tick(
    mut clock: ResMut<GameClock>,
    #[cfg(feature = "dev-mcp")] control: Option<ResMut<dev_mcp::Control>>,
) {
    if !clock.ticking {
        return;
    }
    clock.frame += 1;
    clock.tick += 1;
    #[cfg(feature = "dev-mcp")]
    if let Some(mut control) = control {
        control.finish_tick(clock.frame);
    }
}
