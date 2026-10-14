//! Music and sound effects.
//!
//! The cartridge plays everything through one call, PlaySound(id) (0x2642): ids 0x81..0x97 are songs and
//! jingles, 0x98..0xBA sound effects. The sounds were rendered from the cartridge's SMPS sound driver by
//! `extract/audio.py` into `assets/audio/{music,sfx}/XX.ogg`, described by `assets/audio/sounds.json`
//! ([`SoundBank`]): loop points of the songs, and for effects the driver's priority and the hardware
//! channels they take over.
//!
//! Gameplay code sends [`PlaySound`] messages (object scripts and room events push ids into
//! `Effects::sounds`, which [`drain_script_sounds`] forwards). This module then behaves like the driver:
//! * a song replaces the current song and starts from its beginning, looping from its loop point;
//! * an effect is dropped while a higher-priority effect is still playing, and cuts off effects that use
//!   one of its channels (all effects share FM channels 5 and 6);
//! * 0xBA silences the effects; everything is paused while the game is paused (START, 0xFF0A6C).
//!
//! The room-music timer, RAM word 0xFF0A56 (0x4950), lives with the other script globals in
//! `Effects::globals`: it counts down every game tick except while paused, and at 15 the room's song
//! (`RoomDef::music`, table 0x49DA) starts. Jingles and boss endings set it to restart the room music.
use crate::GameSystems;
use super::{AppState, data::{JsonAsset, JsonLoader, RoomDef}, objects::ops::Effects, pause::Pause, room::CurrentRoom};
use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    audio::{AddAudioSource, AudioSink, AudioSinkPlayback, Decodable, PlaybackMode, Volume},
    prelude::*,
};
use serde::Deserialize;
use std::{collections::HashMap, num::NonZero, sync::Arc, time::Duration};

/// RAM word of the room-music timer.
pub const MUSIC_TIMER: u32 = 0xFF0A56;
/// Sound id that silences every sound effect (its effect has only rests).
pub const SFX_STOP: u32 = 0xBA;

/// Cartridge PlaySound(id).
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaySound(pub u32);

/// Sets the room-music timer ([`MUSIC_TIMER`]) from code that has no access to `Effects`: the room's
/// song starts again when it has counted down to 15 (e.g. 0x78 after the level-up jingle).
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetMusicTimer(pub i32);

/// Level-up: jingle 0x92, room music back after the timer (0xC598).
pub fn level_up_jingle(sound: &mut MessageWriter<PlaySound>, timer: &mut MessageWriter<SetMusicTimer>) {
    sound.write(PlaySound(0x92));
    timer.write(SetMusicTimer(0x78));
}

// ---------------------------------------------------------------------------
// Assets
// ---------------------------------------------------------------------------

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SoundKind {
    Music,
    Sfx,
}

#[derive(Deserialize, Debug, Clone)]
pub struct SoundDef {
    pub id: u32,
    pub kind: SoundKind,
    #[serde(default)]
    pub name: String,
    /// Audio file; `None` for a sound without audible output.
    pub file: Option<String>,
    /// Songs: sample frame the song jumps back to when it reaches its end (absent: plays once).
    pub loop_start: Option<usize>,
    /// Effects: driver priority (a new effect needs at least the priority of the one playing).
    #[serde(default)]
    pub priority: u8,
    /// Effects: hardware channels taken over (0x05/0x06 = FM 5/6, 0x80/0xA0/0xC0 = PSG).
    #[serde(default)]
    pub channels: Vec<u8>,
}

/// `audio/sounds.json`: every sound id.
#[derive(Asset, TypePath, Deserialize, Debug)]
pub struct SoundBank {
    pub sounds: Vec<SoundDef>,
}
impl JsonAsset for SoundBank {}

impl SoundBank {
    pub fn get(&self, id: u32) -> Option<&SoundDef> {
        self.sounds.iter().find(|s| s.id == id)
    }
}

/// Decoded 16-bit PCM, interleaved. Played directly, or with `loop_start` set it loops forever from
/// that sample frame (built per song from the loaded file; the samples are shared).
#[derive(Asset, TypePath, Clone)]
pub struct Pcm {
    pub samples: Arc<[i16]>,
    pub channels: u16,
    pub sample_rate: u32,
    pub loop_start: Option<usize>,
}

/// Plays a [`Pcm`]: once through, then (if looping) from the loop start again, without a gap.
pub struct PcmDecoder {
    pcm: Pcm,
    pos: usize,
}

impl Iterator for PcmDecoder {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.pcm.samples.len() {
            let start = self.pcm.loop_start? * self.pcm.channels as usize;
            if start >= self.pcm.samples.len() {
                return None;
            }
            self.pos = start;
        }
        let s = self.pcm.samples[self.pos];
        self.pos += 1;
        Some(s as f32 / 32768.0)
    }
}

impl bevy::audio::Source for PcmDecoder {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> NonZero<u16> {
        NonZero::new(self.pcm.channels).unwrap_or(NonZero::<u16>::MIN)
    }
    fn sample_rate(&self) -> NonZero<u32> {
        NonZero::new(self.pcm.sample_rate).unwrap_or(NonZero::<u32>::MIN)
    }
    fn total_duration(&self) -> Option<Duration> {
        if self.pcm.loop_start.is_some() {
            return None;
        }
        let frames = self.pcm.samples.len() / self.pcm.channels.max(1) as usize;
        Some(Duration::from_secs_f64(frames as f64 / self.pcm.sample_rate.max(1) as f64))
    }
}

impl Decodable for Pcm {
    type Decoder = PcmDecoder;
    fn decoder(&self) -> PcmDecoder {
        PcmDecoder { pcm: self.clone(), pos: 0 }
    }
}

/// Decodes Ogg Vorbis files into [`Pcm`] up front, so playback (and looping) costs nothing.
#[derive(Default, TypePath)]
pub struct OggPcmLoader;

impl AssetLoader for OggPcmLoader {
    type Asset = Pcm;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(&self, reader: &mut dyn Reader, _: &(), _: &mut LoadContext<'_>) -> Result<Pcm, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        let mut ogg = lewton::inside_ogg::OggStreamReader::new(std::io::Cursor::new(bytes)).map_err(std::io::Error::other)?;
        let mut samples = Vec::new();
        while let Some(packet) = ogg.read_dec_packet_itl().map_err(std::io::Error::other)? {
            samples.extend_from_slice(&packet);
        }
        Ok(Pcm {
            samples: samples.into(),
            channels: ogg.ident_hdr.audio_channels as u16,
            sample_rate: ogg.ident_hdr.audio_sample_rate,
            loop_start: None,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["ogg"]
    }
}

// ---------------------------------------------------------------------------
// Playback
// ---------------------------------------------------------------------------

#[derive(Resource)]
pub struct SoundBankHandle(pub Handle<SoundBank>);

/// Loaded sound files by id. Effects are loaded at start-up and stay resident; songs are loaded when
/// first played and kept (a few MB each).
#[derive(Resource, Default)]
struct SoundFiles(HashMap<u32, Handle<Pcm>>);

/// The song that should be playing (its file may still be loading).
#[derive(Resource, Default)]
pub struct Music {
    pub current: Option<u32>,
    started: bool,
}

/// The most recent sound ids that played (oldest first), for tests and the debug status.
#[derive(Resource, Default)]
pub struct SoundLog(pub std::collections::VecDeque<u32>);

#[derive(Component)]
struct MusicPlayer;

#[derive(Component)]
struct SfxPlayer {
    priority: u8,
    channels: Vec<u8>,
}

fn load_bank(mut commands: Commands, server: Res<AssetServer>) {
    commands.insert_resource(SoundBankHandle(server.load("audio/sounds.json")));
}

/// Loads every effect once the bank is there.
fn preload_effects(
    bank: Res<SoundBankHandle>,
    banks: Res<Assets<SoundBank>>,
    server: Res<AssetServer>,
    mut files: ResMut<SoundFiles>,
    mut done: Local<bool>,
) {
    if *done {
        return;
    }
    let Some(bank) = banks.get(&bank.0) else { return };
    for s in bank.sounds.iter().filter(|s| s.kind == SoundKind::Sfx) {
        if let Some(f) = &s.file {
            files.0.insert(s.id, server.load(f.clone()));
        }
    }
    *done = true;
}

/// Forwards the sound ids pushed by object scripts and room events this tick.
fn drain_script_sounds(mut fx: ResMut<Effects>, mut out: MessageWriter<PlaySound>) {
    for id in fx.sounds.drain(..) {
        out.write(PlaySound(id));
    }
}

/// Room-music timer (0x4950): counts down every tick (not while paused); at 15 the room's song starts.
pub fn music_timer(
    mut fx: ResMut<Effects>,
    mut set: MessageReader<SetMusicTimer>,
    pause: Res<Pause>,
    room: Option<Res<CurrentRoom>>,
    rooms: Res<Assets<RoomDef>>,
    mut out: MessageWriter<PlaySound>,
) {
    for &SetMusicTimer(t) in set.read() {
        fx.globals.insert(MUSIC_TIMER, t);
    }
    let t = fx.global(MUSIC_TIMER);
    if !pause.0.is_empty() || t == 0 {
        return;
    }
    fx.globals.insert(MUSIC_TIMER, t - 1);
    if t - 1 == 15 {
        if let Some(def) = room.and_then(|r| rooms.get(&r.def)) {
            out.write(PlaySound(def.music));
        }
    }
}

fn play_sounds(
    mut commands: Commands,
    mut requests: MessageReader<PlaySound>,
    bank: Res<SoundBankHandle>,
    banks: Res<Assets<SoundBank>>,
    server: Res<AssetServer>,
    mut files: ResMut<SoundFiles>,
    mut music: ResMut<Music>,
    mut log: ResMut<SoundLog>,
    music_players: Query<(Entity, Option<&AudioSink>), With<MusicPlayer>>,
    sfx_players: Query<(Entity, &SfxPlayer, Option<&AudioSink>)>,
) {
    let Some(bank) = banks.get(&bank.0) else {
        requests.clear();
        return;
    };
    for &PlaySound(id) in requests.read() {
        let Some(def) = bank.get(id) else {
            warn!("unknown sound id {id:#x}");
            continue;
        };
        debug!("PlaySound({id:#x}) {}", def.name);
        if log.0.len() >= 32 {
            log.0.pop_front();
        }
        log.0.push_back(id);
        match def.kind {
            SoundKind::Music => {
                for (e, sink) in &music_players {
                    if let Some(s) = sink {
                        s.stop();
                    }
                    commands.entity(e).despawn();
                }
                music.current = Some(id);
                music.started = false;
                if let Some(f) = &def.file {
                    files.0.entry(id).or_insert_with(|| server.load(f.clone()));
                }
            }
            SoundKind::Sfx => {
                // A playing effect of higher priority keeps the channels (Z80 0x08A7).
                let playing: Vec<_> = sfx_players.iter().filter(|(_, _, sink)| sink.is_none_or(|s| !s.empty())).collect();
                if playing.iter().any(|(_, p, _)| p.priority > def.priority) {
                    continue;
                }
                for (e, p, sink) in &playing {
                    if id == SFX_STOP || p.channels.iter().any(|c| def.channels.contains(c)) {
                        if let Some(s) = sink {
                            s.stop();
                        }
                        commands.entity(*e).despawn();
                    }
                }
                if let Some(h) = files.0.get(&id) {
                    commands.spawn((
                        AudioPlayer(h.clone()),
                        PlaybackSettings { mode: PlaybackMode::Despawn, ..default() },
                        SfxPlayer { priority: def.priority, channels: def.channels.clone() },
                    ));
                }
            }
        }
    }
}

/// Starts the current song once its file is loaded.
fn start_music(
    mut commands: Commands,
    mut music: ResMut<Music>,
    files: Res<SoundFiles>,
    bank: Res<SoundBankHandle>,
    banks: Res<Assets<SoundBank>>,
    mut pcms: ResMut<Assets<Pcm>>,
) {
    let Some(id) = music.current.filter(|_| !music.started) else { return };
    let Some(def) = banks.get(&bank.0).and_then(|b| b.get(id)) else { return };
    let Some(pcm) = files.0.get(&id).and_then(|h| pcms.get(h)) else {
        if def.file.is_none() {
            music.started = true;
        }
        return;
    };
    let pcm = Pcm { loop_start: def.loop_start, ..pcm.clone() };
    let handle = pcms.add(pcm);
    commands.spawn((AudioPlayer(handle), PlaybackSettings { mode: PlaybackMode::Despawn, ..default() }, MusicPlayer));
    music.started = true;
}

/// Everything holds while the game is paused (the cartridge pauses the driver, 0x2BEE).
fn pause_audio(pause: Res<Pause>, sinks: Query<&AudioSink>) {
    let paused = !pause.0.is_empty();
    for s in &sinks {
        if paused != s.is_paused() {
            if paused { s.pause() } else { s.play() }
        }
    }
}

/// Silences everything: the cartridge resets (sound driver included) whenever it goes back to the
/// SEGA screen (game over, end of the attract demo, after the ending).
pub fn stop_all(mut commands: Commands, mut music: ResMut<Music>, players: Query<(Entity, &AudioSink)>) {
    for (e, s) in &players {
        s.stop();
        commands.entity(e).despawn();
    }
    *music = Music::default();
}

pub fn plugin(app: &mut App) {
    let volume = if std::env::args().any(|a| a == "--mute") {
        0.0
    } else {
        let args: Vec<String> = std::env::args().collect();
        args.iter()
            .position(|a| a == "--volume")
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(1.0)
    };
    app.insert_resource(GlobalVolume::new(Volume::Linear(volume)))
        .init_asset::<SoundBank>()
        .register_asset_loader(JsonLoader::<SoundBank>::new(&["sounds.json"]))
        .add_audio_source::<Pcm>()
        .register_asset_loader(OggPcmLoader)
        .add_message::<PlaySound>()
        .add_message::<SetMusicTimer>()
        .init_resource::<SoundFiles>()
        .init_resource::<Music>()
        .init_resource::<SoundLog>()
        .add_systems(Startup, load_bank)
        .add_systems(OnEnter(AppState::Sega), stop_all)
        .add_systems(
            FixedUpdate,
            (
                music_timer.in_set(GameSystems::Input).run_if(in_state(AppState::InGame)),
                drain_script_sounds.in_set(GameSystems::Finish),
            ),
        )
        .add_systems(Update, (preload_effects, play_sounds, start_music, pause_audio).chain());
}
