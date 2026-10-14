# Cadash (Bevy remake)

A native Rust / **Bevy 0.20** re-implementation of Taito's *Cadash* for the Mega Drive. No game data is
included: graphics, level data, text, game tables and sound are extracted from your own copy of the
cartridge into ordinary files under `assets/` by the scripts in `extract/`. The game itself never reads the
ROM. Behaviour (physics, enemy scripts, timings) follows reverse-engineered specifications that are not
part of this repository.

**Written by Claude** (Anthropic's AI model, in Claude Code). Claude reverse engineered the cartridge with the **mdx** Mega Drive analysis tool (disassembly, emulator boots and traces, memory watches, graphics and audio extraction), wrote the extraction scripts in `extract/`, and wrote the Rust code, checking behaviour against the original by comparing emulator frames and RAM with the remake. The project started from mdx's Bevy template, which includes the developer MCP server (`--features dev-mcp`) used to drive the game and take screenshots.

## Getting started

1. Get the **Cadash (USA, Korea)** cartridge image (`Cadash (USA, Korea) (En).md`, SHA-256
   `85e07824…512afd8`; other releases are refused).
2. Install Python 3 with `pillow`, `numpy` and `soundfile` (`pip install pillow numpy soundfile`), and build
   [mdx](https://github.com/bombpersons/mdx), which renders the music and sound effects by running the
   cartridge's own sound driver.
3. Extract the assets (a few minutes, mostly audio):

   ```sh
   python3 extract/extract_assets.py "Cadash (USA, Korea) (En).md" --mdx /path/to/mdx
   ```

   `--no-audio` skips the sound (the game then runs silently).
4. Build and play:

```sh
cargo run                      # play
cargo run -- --skip-intro      # straight into the castle
cargo build --release
```

| Mega Drive | Keyboard | Gamepad |
|---|---|---|
| D-pad | Arrow keys | D-pad / left stick |
| A (jump) | Z | West |
| B (attack, talk, Mage: hold to charge magic) | X | South |
| C (jump) | C | East |
| Start | Enter | Start |

Up enters doors, stairs and portals.

## Architecture

The game is ordinary Bevy ECS. Simulation runs in `FixedUpdate` at 60 Hz (`GameSystems::{Input, Game,
Animate, Camera, Finish}`), rendering is plain `Sprite`s drawn into a 320×224 target.

| module (`src/game/`) | responsibility |
|---|---|
| `data.rs` | serde data assets + a generic JSON asset loader (`*.room.json`, `*.sheet.json`, `*.objects.json`, …) |
| `coords.rs` | world space (x right, y down, 16.16 fixed point) → Bevy transforms; draw layers |
| `input.rs` | joypad with the cartridge's two-read debounce |
| `anim.rs` | sprite-sheet animation (frame durations, loop points, origins, hit boxes) |
| `collision.rs` | block collision: solids, one-way surfaces/slopes, 8-px snapping (shared by all actors) |
| `player.rs`, `player_look.rs` | player state machine, stats/levels, equipment-driven palette & graphics |
| `room.rs`, `transition.rs`, `camera.rs` | room layers, parallax, palette cycling; exits & fades; camera |
| `objects/` | data-driven object scripts (states → behaviour/init/condition opcodes), spawning, combat, bosses |
| `dialog.rs`, `hud_message.rs`, `ui.rs` | text windows/typewriter, status-bar messages, HUD and bitmap font |
| `frontend.rs`, `lifecycle.rs` | SEGA/title/character select/name entry; death, CONTINUE?, GAME OVER |
| `audio.rs` | `PlaySound(id)` messages → songs (gapless intro + loop) and effects (driver priorities/channels); room-music timer |

## Assets

`assets/` is written by `extract/extract_assets.py`, which checks the ROM, recreates the layout the
individual scripts expect under `target/extract-root/` and runs them in order (intermediate files go to
`target/extract-root/extract/out/`). Everything is
plain PNG/JSON: rooms (`rooms/`), object scripts and sprite atlases (`objects/`), player sheets
(`sprites/player/`), text (`text/`), UI (`ui/`), screens (`screens/`), tables (`data/`) and sound
(`audio/`: OGG files plus `sounds.json` with loop points, effect priorities and channels). Content can be
edited or extended there without code changes.

## Developer options

`--room N --x X --y Y --facing left|right --class fighter|mage --hp N --gear a,b,c,d,e,f --no-intro
--skip-intro` start anywhere for testing; `--mute` / `--volume X` set the sound volume. With `--features dev-mcp`, `game_status` also reports the
player and object state (`debug`).

## Agent driving

Build with `dev-mcp` and launch the binary with `--mcp` as a **stdio MCP server**.
Bevy logs go to stderr; stdout is reserved for JSON-RPC. The window still opens.
Developer mode starts with simulation paused, while rendering/window events
continue. This makes `game_step` followed by `game_screenshot` reproducible.

| Tool | Arguments | Behavior |
|---|---|---|
| `game_status` | `{}` | Frame, pause state, dimensions, sampled controller |
| `game_step` | `{"frames":30,"buttons":["right","b"]}` | Holds buttons for exactly30 ticks, releases injected input, then freezes |
| `game_set_input` | `{"buttons":["a"]}` | Replaces held virtual input; `[]` releases it |
| `game_pause` | `{"paused":false}` | Resumes continuous simulation / manual play |
| `game_screenshot` | `{"target":"screen"}` | Inline PNG from the actual GPU-rendered320×224 game texture |
| `game_screenshot` | `{"target":"window"}` | Actual window-rendered PNG, including scaling and letterboxing |

Buttons are `up/down/left/right/a/b/c/start`; frame requests are1..3600. Real
device input is merged with virtual input. Keep physical devices released during
automated driving. Requests are serialized, bounded and cancellable. Cancelled
steps release virtual input on the next fixed-tick boundary. Closing MCP stdin
closes the developer-owned application. Screenshot metadata records the request
frame; while continuous simulation is running, GPU readback is asynchronous.
Pause first when the image must represent a stable simulation state.
The first screenshot waits for actual render targets and compiled sprite
pipelines to complete a camera pass. Developer windows continue updating while
unfocused, including remote-desktop sessions. For elapsed simulation time use
`GameClock.frame / 60.0`; Bevy's wall/virtual elapsed clocks are not pause-aware
under developer stepping.

The last sampled `ControllerState` describes the last simulated tick. After a
step, its held indicators can remain visible while frozen even though injected
input has been released; the next tick updates the normal release edge.

All server code and direct MCP/Tokio/serialization dependencies are feature-gated.
An ordinary `cargo build --release` has no MCP endpoint. `--mcp` without the
feature exits with an explanatory error rather than silently enabling tooling.

## Checks

```sh
cargo test
cargo test --features dev-mcp
cargo check --release
```

Keep game-specific assets, state, persistence and gameplay in separate modules.
This shell supplies the desktop viewport, controller contract and optional
developer bridge; it does not impose an emulator or a generic game framework.
