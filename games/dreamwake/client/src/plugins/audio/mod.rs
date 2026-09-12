//! Original synthesized audio. No downloaded samples or external asset paths.
use crate::{DreamPreferences, DreamView};
use bevy::{
    audio::{AudioSinkPlayback, Volume},
    prelude::*,
};
use dreamwake_sim::RunPhase;
use std::f32::consts::TAU;

pub struct DreamAudioPlugin;
impl Plugin for DreamAudioPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup_audio)
            .add_systems(Update, update_audio);
    }
}
#[derive(Resource)]
struct SoundBank {
    slash: Handle<AudioSource>,
    hit: Handle<AudioSource>,
    dash: Handle<AudioSource>,
    cast: Handle<AudioSource>,
    warning: Handle<AudioSource>,
    reward: Handle<AudioSource>,
    defeat: Handle<AudioSource>,
    victory: Handle<AudioSource>,
    ready: Handle<AudioSource>,
}
#[derive(Resource)]
struct AudioHistory {
    phase: RunPhase,
    tick: u32,
    hp: f32,
    attack: f32,
    dash: f32,
    cooldowns: [f32; 4],
    warnings: usize,
    kills: u32,
    last_warning: f32,
}
#[derive(Component)]
struct DreamSound;
#[derive(Component)]
struct Music;

fn wav(duration: f32, sample: impl Fn(f32) -> f32) -> AudioSource {
    let rate = 22050_u32;
    let frames = (duration * rate as f32) as usize;
    let size = frames as u32 * 2;
    let mut bytes = Vec::with_capacity(size as usize + 44);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(size + 36).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&rate.to_le_bytes());
    bytes.extend_from_slice(&(rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&size.to_le_bytes());
    for i in 0..frames {
        let value = (sample(i as f32 / rate as f32).clamp(-1.0, 1.0) * 27000.0) as i16;
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    AudioSource {
        bytes: bytes.into(),
    }
}
fn tone(t: f32, f: f32) -> f32 {
    (TAU * f * t).sin()
}
fn env(t: f32, duration: f32) -> f32 {
    (t / 0.006).min(1.0) * (1.0 - t / duration).max(0.0).powf(2.2)
}
fn chime(t: f32, notes: &[f32], spacing: f32) -> f32 {
    notes
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let local = t - i as f32 * spacing;
            if local < 0.0 {
                0.0
            } else {
                (tone(local, *f) + 0.18 * tone(local, f * 2.01))
                    * (-local * 5.0).exp()
                    * (local / 0.008).min(1.0)
            }
        })
        .sum::<f32>()
        * 0.3
}
fn setup_audio(mut commands: Commands, mut sources: ResMut<Assets<AudioSource>>) {
    let bank = SoundBank {
        slash: sources.add(wav(0.16, |t| {
            (tone(t, 680.0 - t * 2400.0) + 0.3 * tone(t, 1493.0)) * env(t, 0.16) * 0.4
        })),
        hit: sources.add(wav(0.17, |t| {
            (tone(t, 130.0 - t * 420.0) + 0.18 * tone(t, 1073.0)) * env(t, 0.17) * 0.6
        })),
        dash: sources.add(wav(0.34, |t| {
            (tone(t, 220.0 + t * 1000.0) + 0.2 * tone(t, 880.0)) * env(t, 0.34) * 0.36
        })),
        cast: sources.add(wav(0.45, |t| chime(t, &[587.33, 880.0, 1174.66], 0.055))),
        warning: sources.add(wav(0.38, |t| {
            (tone(t, 196.0) + tone(t, 207.65)) * (0.6 + 0.4 * tone(t, 12.0)) * env(t, 0.38) * 0.3
        })),
        reward: sources.add(wav(1.4, |t| {
            chime(t, &[293.66, 440.0, 587.33, 739.99, 880.0], 0.12)
        })),
        defeat: sources.add(wav(2.0, |t| {
            chime(t, &[440.0, 349.23, 293.66, 146.83], 0.24)
        })),
        victory: sources.add(wav(2.4, |t| {
            chime(
                t,
                &[293.66, 369.99, 440.0, 587.33, 739.99, 880.0, 1174.66],
                0.16,
            )
        })),
        ready: sources.add(wav(0.28, |t| chime(t, &[1174.66, 1567.98], 0.07) * 0.3)),
    };
    // Four suspended chords, a quiet bell ostinato, and a soft heartbeat bass.
    // Integer cycle lengths and boundary fades make the 16-second score loop cleanly.
    let music = sources.add(wav(16.0, |t| {
        let chord = ((t / 4.0) as usize).min(3);
        let notes = [
            [146.83, 220.0, 261.63, 329.63],
            [130.81, 196.0, 246.94, 293.66],
            [174.61, 220.0, 261.63, 349.23],
            [110.0, 164.81, 220.0, 293.66],
        ][chord];
        let local = t % 4.0;
        let swell = (local / 0.65).min(1.0) * ((4.0 - local) / 0.9).min(1.0);
        let pad = notes
            .iter()
            .map(|f| tone(t, *f) + 0.18 * tone(t, f * 1.003))
            .sum::<f32>()
            * 0.04
            * swell;
        let bell_time = t % 0.5;
        let bell = tone(bell_time, notes[(t * 2.0) as usize % 4] * 4.0)
            * (-bell_time * 10.0).exp()
            * (bell_time / 0.008).min(1.0)
            * 0.045;
        let pulse = tone(t % 1.0, notes[0] / 2.0) * (-(t % 1.0) * 8.0).exp() * 0.05;
        (pad + bell + pulse) * (t / 0.1).min(1.0) * ((16.0 - t) / 0.15).min(1.0)
    }));
    commands.spawn((
        AudioPlayer::new(music),
        PlaybackSettings::LOOP.with_volume(Volume::Linear(0.52)),
        DreamSound,
        Music,
    ));
    commands.insert_resource(bank);
    commands.insert_resource(AudioHistory {
        phase: RunPhase::Intro,
        tick: 0,
        hp: 0.0,
        attack: 0.0,
        dash: 0.0,
        cooldowns: [0.0; 4],
        warnings: 0,
        kills: 0,
        last_warning: 0.0,
    });
}
fn play(commands: &mut Commands, source: &Handle<AudioSource>, volume: f32, muted: bool) {
    commands.spawn((
        AudioPlayer::new(source.clone()),
        PlaybackSettings {
            muted,
            ..PlaybackSettings::DESPAWN.with_volume(Volume::Linear(volume))
        },
        DreamSound,
    ));
}
fn update_audio(
    mut commands: Commands,
    view: Res<DreamView>,
    prefs: Res<DreamPreferences>,
    bank: Res<SoundBank>,
    mut history: ResMut<AudioHistory>,
    runtime: Option<NonSendMut<crate::plugins::network::Runtime>>,
    time: Res<Time<Real>>,
    mut sinks: Query<(&mut AudioSink, Option<&Music>), With<DreamSound>>,
) {
    if let Some(mut runtime) = runtime
        && let Some(session) = &mut runtime.prediction
    {
        let count = std::mem::take(&mut session.events.sounds);
        for _ in 0..count {
            play(&mut commands, &bank.cast, 0.5, prefs.muted);
        }
    }
    for (mut sink, music) in &mut sinks {
        if prefs.muted {
            sink.mute();
        } else {
            sink.unmute();
        }
        if music.is_some() {
            let volume = if prefs.paused || prefs.build_open {
                0.23
            } else if view.0.room == 9 {
                0.75
            } else {
                0.52
            };
            sink.set_volume(Volume::Linear(volume));
        }
    }
    let s = &view.0;
    if s.tick == history.tick && s.phase == history.phase {
        return;
    }
    if s.phase != history.phase {
        let sound = match s.phase {
            RunPhase::Reward | RunPhase::Rest | RunPhase::Transition => Some(&bank.reward),
            RunPhase::Victory => Some(&bank.victory),
            RunPhase::Defeat => Some(&bank.defeat),
            RunPhase::Combat => Some(&bank.cast),
            _ => None,
        };
        if let Some(sound) = sound {
            play(&mut commands, sound, 0.6, prefs.muted);
        }
    }
    if s.phase == RunPhase::Combat && s.tick > history.tick {
        if s.hero.attack_cooldown > history.attack + 0.05 {
            play(&mut commands, &bank.slash, 0.45, prefs.muted);
        }
        if s.hero.dash_cooldown > history.dash + 0.1 {
            play(&mut commands, &bank.dash, 0.55, prefs.muted);
        }
        if s.hero.hp < history.hp || s.kills > history.kills {
            play(&mut commands, &bank.hit, 0.4, prefs.muted);
        }
        let cast = s.hero.memories.iter().enumerate().any(|(i, m)| {
            m.kind != dreamwake_sim::MemoryKind::Starfall
                && m.cooldown > history.cooldowns[i] + 0.15
        });
        if cast {
            play(&mut commands, &bank.cast, 0.5, prefs.muted);
        }
        let ready = s
            .hero
            .memories
            .iter()
            .enumerate()
            .any(|(i, m)| m.cooldown <= 0.0 && history.cooldowns[i] > 0.0);
        if ready {
            play(&mut commands, &bank.ready, 0.24, prefs.muted);
        }
        let warnings = s.enemies.iter().filter(|e| e.windup > 0.0).count();
        if warnings > history.warnings && time.elapsed_secs() > history.last_warning + 0.35 {
            play(&mut commands, &bank.warning, 0.5, prefs.muted);
            history.last_warning = time.elapsed_secs();
        }
        history.warnings = warnings;
    }
    history.tick = s.tick;
    history.phase = s.phase;
    history.hp = s.hero.hp;
    history.kills = s.kills;
    history.attack = s.hero.attack_cooldown;
    history.dash = s.hero.dash_cooldown;
    history.cooldowns = std::array::from_fn(|i| s.hero.memories[i].cooldown);
}
