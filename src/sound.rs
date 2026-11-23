//! Optional alert sounds, synthesized in code: no sound files and no audio
//! libraries. Windows plays the generated WAV with `PlaySound`; Linux hands
//! it to whichever player is installed (`paplay`, `pw-play` or `aplay`).
//!
//! Sounds only play for the important moments: errors, alerts that need the
//! operator, homing complete and wind done. Each of those can use any sound
//! from the library below; presets fill all four at once.
//!
//! The ALR-67 tones use the frequencies and timings documented for the real
//! AN/ALR-67(V) audio by the VRS Superbug F/A-18 simulator wiki ("ALR-67 Audio
//! Tones"); the falling four-step "special" tone comes from community notes
//! and the Heatblur F-14 manual. The waveform isn't documented, so a
//! band-limited square wave (a typical avionics tone) is used.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const RATE: u32 = 22_050;
/// The same alert won't repeat faster than this.
const MIN_REPEAT: Duration = Duration::from_millis(1500);

/// The moments that can make a sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Alert {
    /// Something went wrong: job failed, board halted, connection lost.
    Error,
    /// The machine needs you: job paused, or the board stopped answering.
    Attention,
    /// Homing finished.
    Homed,
    /// The wind finished.
    WindDone,
}

impl Alert {
    pub const ALL: [Alert; 4] = [Alert::Error, Alert::Attention, Alert::Homed, Alert::WindDone];

    pub fn label(self) -> &'static str {
        match self {
            Alert::Error => "Error",
            Alert::Attention => "Alert",
            Alert::Homed => "Homing done",
            Alert::WindDone => "Wind done",
        }
    }
}

/// Every sound Typhoon can make.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sound {
    /// Missile launch: 455 / 555 Hz alternating, 0.166 s each (fast warble).
    RwrLaunch,
    /// Single target track (lock): 455 / 555 Hz alternating, 0.5 s each (slow warble).
    RwrTrack,
    /// Status change / new threat: one 455 Hz tone for 0.33 s.
    RwrStatus,
    /// Special event: 550, 520, 490, 470 Hz, 0.083 s each.
    RwrSpecial,
    /// Airborne interceptor: 555 Hz on/off, 0.5 s each.
    RwrAi,
    /// AAA: 555 Hz on/off every 0.166 s.
    RwrAaa,
    /// Stepper-motor "beep beep", in the style of a Bambu Lab A1 error.
    A1Error,
    A1Notice,
    A1Homed,
    A1Done,
    ChimeFall,
    ChimeDouble,
    ChimeBell,
    ChimeArpeggio,
    BeepTriple,
    BeepDouble,
    BeepSingle,
    BeepRise,
    Silent,
}

impl Sound {
    pub const ALL: [Sound; 19] = [
        Sound::RwrLaunch,
        Sound::RwrTrack,
        Sound::RwrStatus,
        Sound::RwrSpecial,
        Sound::RwrAi,
        Sound::RwrAaa,
        Sound::A1Error,
        Sound::A1Notice,
        Sound::A1Homed,
        Sound::A1Done,
        Sound::ChimeFall,
        Sound::ChimeDouble,
        Sound::ChimeBell,
        Sound::ChimeArpeggio,
        Sound::BeepTriple,
        Sound::BeepDouble,
        Sound::BeepSingle,
        Sound::BeepRise,
        Sound::Silent,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Sound::RwrLaunch => "ALR-67 · Missile launch",
            Sound::RwrTrack => "ALR-67 · Lock (STT)",
            Sound::RwrStatus => "ALR-67 · Status change",
            Sound::RwrSpecial => "ALR-67 · Special (4-tone)",
            Sound::RwrAi => "ALR-67 · Airborne interceptor",
            Sound::RwrAaa => "ALR-67 · AAA",
            Sound::A1Error => "Bambu A1-style · Error beeps",
            Sound::A1Notice => "Bambu A1-style · Notice",
            Sound::A1Homed => "Bambu A1-style · Rising pair",
            Sound::A1Done => "Bambu A1-style · Finished",
            Sound::ChimeFall => "Chime · Falling",
            Sound::ChimeDouble => "Chime · Double",
            Sound::ChimeBell => "Chime · Bell",
            Sound::ChimeArpeggio => "Chime · Arpeggio",
            Sound::BeepTriple => "Beep · Triple low",
            Sound::BeepDouble => "Beep · Double",
            Sound::BeepSingle => "Beep · Single",
            Sound::BeepRise => "Beep · Rising",
            Sound::Silent => "Silent",
        }
    }
}

/// Which sound each alert uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlertSounds {
    pub error: Sound,
    pub attention: Sound,
    pub homed: Sound,
    pub wind_done: Sound,
}

impl Default for AlertSounds {
    fn default() -> Self {
        Preset::Alr67.sounds()
    }
}

impl AlertSounds {
    pub fn get(&self, alert: Alert) -> Sound {
        match alert {
            Alert::Error => self.error,
            Alert::Attention => self.attention,
            Alert::Homed => self.homed,
            Alert::WindDone => self.wind_done,
        }
    }

    pub fn get_mut(&mut self, alert: Alert) -> &mut Sound {
        match alert {
            Alert::Error => &mut self.error,
            Alert::Attention => &mut self.attention,
            Alert::Homed => &mut self.homed,
            Alert::WindDone => &mut self.wind_done,
        }
    }

    /// The preset these choices match, if any.
    pub fn preset(&self) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.sounds() == *self)
    }
}

/// Ready-made choices for all four alerts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Alr67,
    BambuA1,
    Chimes,
    Beeps,
}

impl Preset {
    pub const ALL: [Preset; 4] = [Preset::Alr67, Preset::BambuA1, Preset::Chimes, Preset::Beeps];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Alr67 => "ALR-67 RWR tones",
            Preset::BambuA1 => "Bambu A1-style",
            Preset::Chimes => "Soft chimes",
            Preset::Beeps => "Simple beeps",
        }
    }

    pub fn sounds(self) -> AlertSounds {
        let (error, attention, homed, wind_done) = match self {
            // Launch warble for errors, lock tone for alerts, the status-change
            // tone when homing completes, the special 4-tone for wind done.
            Preset::Alr67 => (Sound::RwrLaunch, Sound::RwrTrack, Sound::RwrStatus, Sound::RwrSpecial),
            Preset::BambuA1 => (Sound::A1Error, Sound::A1Notice, Sound::A1Homed, Sound::A1Done),
            Preset::Chimes => (Sound::ChimeFall, Sound::ChimeDouble, Sound::ChimeBell, Sound::ChimeArpeggio),
            Preset::Beeps => (Sound::BeepTriple, Sound::BeepDouble, Sound::BeepSingle, Sound::BeepRise),
        };
        AlertSounds {
            error,
            attention,
            homed,
            wind_done,
        }
    }
}

/// Plays alerts, skipping repeats of the same alert in quick succession.
#[derive(Default)]
pub struct Player {
    last: HashMap<Alert, Instant>,
}

impl Player {
    pub fn alert(&mut self, sound: Sound, alert: Alert, volume: f32) {
        let now = Instant::now();
        if self.last.get(&alert).is_some_and(|t| now - *t < MIN_REPEAT) {
            return;
        }
        self.last.insert(alert, now);
        play(sound, volume);
    }
}

/// Play right away (also used by the Preferences "try" buttons).
pub fn play(sound: Sound, volume: f32) {
    if sound == Sound::Silent {
        return;
    }
    play_wav(wav(&synthesize(sound, volume)));
}

// ----------------------------------------------------------------- synthesis

#[derive(Clone, Copy)]
enum Timbre {
    /// Plain sine.
    Sine,
    /// Band-limited square wave: odd harmonics up to the 7th (avionics tone).
    Square,
    /// A stepper motor singing: strong harmonics plus a little mechanical buzz.
    Motor,
}

struct Synth {
    out: Vec<f32>,
    phase: f32,
}

impl Synth {
    fn new() -> Self {
        Self {
            out: Vec::new(),
            phase: 0.0,
        }
    }

    fn samples(seconds: f32) -> usize {
        (seconds * RATE as f32).round() as usize
    }

    fn silence(&mut self, seconds: f32) -> &mut Self {
        self.out.extend(std::iter::repeat_n(0.0, Self::samples(seconds)));
        self.phase = 0.0;
        self
    }

    /// One tone segment. Consecutive `steps` keep their phase, so alternating
    /// pitches switch without a click; `fade_in`/`fade_out` soften the ends.
    fn step(&mut self, freq: f32, seconds: f32, timbre: Timbre, fade_in: bool, fade_out: bool) -> &mut Self {
        let n = Self::samples(seconds);
        let fade = Self::samples(0.003).max(1);
        for i in 0..n {
            self.phase += std::f32::consts::TAU * freq / RATE as f32;
            if self.phase > std::f32::consts::TAU * 1000.0 {
                self.phase -= std::f32::consts::TAU * 1000.0;
            }
            let p = self.phase;
            let s = match timbre {
                Timbre::Sine => p.sin(),
                Timbre::Square => {
                    (p.sin() + (3.0 * p).sin() / 3.0 + (5.0 * p).sin() / 5.0 + (7.0 * p).sin() / 7.0) / 1.15
                }
                Timbre::Motor => {
                    // Rich harmonics carry low notes on small laptop speakers;
                    // the slow amplitude wobble gives the motor its growl.
                    let t = i as f32 / RATE as f32;
                    let growl = 1.0 + 0.2 * (std::f32::consts::TAU * 29.0 * t).sin();
                    (p.sin()
                        + 0.6 * (2.0 * p).sin()
                        + 0.45 * (3.0 * p).sin()
                        + 0.3 * (4.0 * p).sin()
                        + 0.2 * (5.0 * p).sin()
                        + 0.12 * (6.0 * p).sin())
                        / 3.0
                        * growl
                }
            };
            let mut env = 1.0;
            if fade_in && i < fade {
                env = i as f32 / fade as f32;
            }
            if fade_out && n - 1 - i < fade {
                env = env.min((n - 1 - i) as f32 / fade as f32);
            }
            self.out.push(s * env);
        }
        self
    }

    /// A complete tone with soft edges.
    fn tone(&mut self, freq: f32, seconds: f32, timbre: Timbre) -> &mut Self {
        self.step(freq, seconds, timbre, true, true)
    }

    /// A bell-like note that rings out.
    fn bell(&mut self, freq: f32, seconds: f32) -> &mut Self {
        let start = self.out.len();
        self.tone(freq, seconds, Timbre::Sine);
        let n = self.out.len() - start;
        for (i, s) in self.out[start..].iter_mut().enumerate() {
            *s *= (-5.0 * i as f32 / n as f32).exp();
        }
        self
    }

    /// Alternate between pitches every `hold` seconds, `count` steps in all.
    fn alternate(&mut self, freqs: &[f32], hold: f32, count: usize, timbre: Timbre) -> &mut Self {
        for k in 0..count {
            let f = freqs[k % freqs.len()];
            self.step(f, hold, timbre, k == 0, k + 1 == count);
        }
        self
    }

    /// The same tone switched on and off, `count` times.
    fn pulses(&mut self, freq: f32, on: f32, off: f32, count: usize, timbre: Timbre) -> &mut Self {
        for _ in 0..count {
            self.tone(freq, on, timbre).silence(off);
        }
        self
    }
}

/// The sound as 16-bit mono samples at 22.05 kHz.
pub fn synthesize(sound: Sound, volume: f32) -> Vec<i16> {
    let gain = volume.clamp(0.0, 1.0) * 0.8 * i16::MAX as f32;
    render(sound).iter().map(|x| (x.clamp(-1.0, 1.0) * gain) as i16).collect()
}

/// The sound as floating-point samples in -1..=1 (before volume).
fn render(sound: Sound) -> Vec<f32> {
    use Timbre::{Motor, Square};
    let mut s = Synth::new();
    match sound {
        // ALR-67, as documented (see the module comment).
        Sound::RwrLaunch => {
            s.alternate(&[455.0, 555.0], 0.166, 8, Square);
        }
        Sound::RwrTrack => {
            s.alternate(&[455.0, 555.0], 0.5, 4, Square);
        }
        Sound::RwrStatus => {
            s.tone(455.0, 0.33, Square);
        }
        Sound::RwrSpecial => {
            s.alternate(&[550.0, 520.0, 490.0, 470.0], 0.083, 4, Square);
        }
        Sound::RwrAi => {
            s.pulses(555.0, 0.5, 0.5, 2, Square);
        }
        Sound::RwrAaa => {
            s.pulses(555.0, 0.166, 0.166, 4, Square);
        }

        // Bambu A1-style: low, ominous motor tones. The A1's own error beep
        // isn't documented, so this is a "beep beep" in that style, not a copy.
        Sound::A1Error => {
            // Beep-beep on G3, then again a semitone lower (F#3).
            for f in [196.0, 185.0] {
                s.tone(f, 0.17, Motor).silence(0.09).tone(f, 0.17, Motor).silence(0.28);
            }
        }
        Sound::A1Notice => {
            // A falling major third: A3 to F3.
            s.tone(220.0, 0.2, Motor).silence(0.08).tone(175.0, 0.28, Motor);
        }
        Sound::A1Homed => {
            // A short low rise: E3 to A3.
            s.tone(165.0, 0.12, Motor).silence(0.03).tone(220.0, 0.2, Motor);
        }
        Sound::A1Done => {
            // C minor arpeggio, ending on a held C4.
            for f in [131.0, 156.0, 196.0] {
                s.tone(f, 0.15, Motor).silence(0.03);
            }
            s.tone(262.0, 0.45, Motor);
        }

        Sound::ChimeFall => {
            s.bell(659.0, 0.35).bell(523.0, 0.55);
        }
        Sound::ChimeDouble => {
            s.bell(880.0, 0.3).bell(880.0, 0.45);
        }
        Sound::ChimeBell => {
            s.bell(1047.0, 0.4);
        }
        Sound::ChimeArpeggio => {
            for f in [523.0, 659.0, 784.0] {
                s.bell(f, 0.18);
            }
            s.bell(1047.0, 0.7);
        }

        Sound::BeepTriple => {
            s.pulses(400.0, 0.15, 0.08, 3, Timbre::Sine);
        }
        Sound::BeepDouble => {
            s.pulses(800.0, 0.12, 0.08, 2, Timbre::Sine);
        }
        Sound::BeepSingle => {
            s.tone(1000.0, 0.1, Timbre::Sine);
        }
        Sound::BeepRise => {
            s.tone(1000.0, 0.1, Timbre::Sine).silence(0.05).tone(1500.0, 0.2, Timbre::Sine);
        }
        Sound::Silent => {}
    }
    s.out
}

/// A complete mono 16-bit PCM WAV file.
pub fn wav(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&RATE.to_le_bytes());
    b.extend_from_slice(&(RATE * 2).to_le_bytes()); // bytes per second
    b.extend_from_slice(&2u16.to_le_bytes()); // block align
    b.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    b
}

// ------------------------------------------------------------------ playback

#[cfg(windows)]
fn play_wav(bytes: Vec<u8>) {
    #[link(name = "winmm")]
    unsafe extern "system" {
        fn PlaySoundW(sound: *const u16, module: *mut std::ffi::c_void, flags: u32) -> i32;
    }
    const SND_SYNC: u32 = 0x0000;
    const SND_NODEFAULT: u32 = 0x0002;
    const SND_MEMORY: u32 = 0x0004;
    // Played synchronously on its own thread, so the buffer outlives the sound.
    std::thread::spawn(move || {
        // SAFETY: with SND_MEMORY the first argument points to a complete WAV
        // image, which `bytes` holds until PlaySoundW returns.
        unsafe {
            PlaySoundW(
                bytes.as_ptr().cast(),
                std::ptr::null_mut(),
                SND_MEMORY | SND_NODEFAULT | SND_SYNC,
            );
        }
    });
}

#[cfg(target_os = "linux")]
fn play_wav(bytes: Vec<u8>) {
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    std::thread::spawn(move || {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("typhoon-{}-{n}.wav", std::process::id()));
        if std::fs::write(&path, &bytes).is_err() {
            return;
        }
        // PulseAudio/PipeWire first, then plain ALSA.
        for (player, args) in [("paplay", &[][..]), ("pw-play", &[][..]), ("aplay", &["-q"][..])] {
            let played = Command::new(player)
                .args(args)
                .arg(&path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if played {
                break;
            }
        }
        let _ = std::fs::remove_file(&path);
    });
}

#[cfg(not(any(windows, target_os = "linux")))]
fn play_wav(_bytes: Vec<u8>) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn seconds(samples: usize) -> f32 {
        samples as f32 / RATE as f32
    }

    /// Frequency from zero crossings over a slice.
    fn freq(s: &[i16]) -> f32 {
        let crossings = s.windows(2).filter(|p| (p[0] < 0) != (p[1] < 0)).count();
        crossings as f32 / 2.0 / seconds(s.len())
    }

    #[test]
    fn every_sound_is_short_faded_and_not_clipped() {
        for sound in Sound::ALL.into_iter().filter(|s| *s != Sound::Silent) {
            let s = synthesize(sound, 1.0);
            let secs = seconds(s.len());
            assert!((0.05..=2.2).contains(&secs), "{sound:?}: {secs}s");
            let peak = s.iter().map(|x| x.unsigned_abs()).max().unwrap();
            assert!(peak > 8_000, "{sound:?} too quiet: {peak}");
            // Before the final clamp the waveform must stay within full scale.
            let raw_peak = render(sound).iter().fold(0.0_f32, |m, x| m.max(x.abs()));
            assert!(raw_peak <= 1.0 + 1e-6, "{sound:?} clips: {raw_peak}");
            assert!(s.last().unwrap().unsigned_abs() < 1_000, "{sound:?} ends with a click");
        }
        assert!(synthesize(Sound::Silent, 1.0).is_empty());
        assert!(synthesize(Sound::RwrLaunch, 0.0).iter().all(|x| *x == 0));
    }

    /// The ALR-67 tones match the documented frequencies and timings.
    #[test]
    fn alr67_matches_documented_tones() {
        let step = |secs: f32| (secs * RATE as f32).round() as usize;
        let near = |got: f32, want: f32| (got - want).abs() / want < 0.02;

        // Missile launch: 455 / 555 Hz alternating, 0.166 s each, 8 steps.
        let ml = synthesize(Sound::RwrLaunch, 1.0);
        assert_eq!(ml.len(), 8 * step(0.166));
        for k in 0..8 {
            let start = k * step(0.166) + step(0.02);
            let f = freq(&ml[start..start + step(0.12)]);
            let want = if k % 2 == 0 { 455.0 } else { 555.0 };
            assert!(near(f, want), "launch step {k}: {f} Hz, want {want}");
        }

        // Lock: same pitches, 0.5 s each.
        let stt = synthesize(Sound::RwrTrack, 1.0);
        assert_eq!(stt.len(), 4 * step(0.5));
        assert!(near(freq(&stt[step(0.05)..step(0.45)]), 455.0));
        assert!(near(freq(&stt[step(0.55)..step(0.95)]), 555.0));

        // Status change: a single 455 Hz tone for 0.33 s.
        let status = synthesize(Sound::RwrStatus, 1.0);
        assert_eq!(status.len(), step(0.33));
        assert!(near(freq(&status[step(0.02)..step(0.31)]), 455.0));

        // Special: 550, 520, 490, 470 Hz falling, 0.083 s each.
        let special = synthesize(Sound::RwrSpecial, 1.0);
        assert_eq!(special.len(), 4 * step(0.083));
        for (k, want) in [550.0, 520.0, 490.0, 470.0].into_iter().enumerate() {
            let start = k * step(0.083) + step(0.005);
            let f = freq(&special[start..start + step(0.07)]);
            assert!((f - want).abs() < 20.0, "special step {k}: {f} Hz, want {want}");
        }
    }

    /// Fundamental frequency by autocorrelation (robust to strong harmonics).
    fn pitch(s: &[i16]) -> f32 {
        let x: Vec<f32> = s.iter().map(|v| *v as f32).collect();
        let (lo, hi) = ((RATE / 600) as usize, (RATE / 80) as usize);
        let best = (lo..=hi)
            .max_by(|a, b| {
                let c = |lag: usize| x.iter().zip(&x[lag..]).map(|(p, q)| p * q).sum::<f32>();
                c(*a).total_cmp(&c(*b))
            })
            .unwrap();
        RATE as f32 / best as f32
    }

    /// The A1-style error is low: beep-beep on G3, then a semitone lower.
    #[test]
    fn a1_error_is_low_and_falls() {
        let s = synthesize(Sound::A1Error, 1.0);
        let ms = |t: f32| (t * RATE as f32) as usize;
        let first = pitch(&s[ms(0.02)..ms(0.15)]);
        // Second pair starts after two beeps, a gap and the pause: 0.17+0.09+0.17+0.28.
        let second_start = ms(0.71) + ms(0.02);
        let second = pitch(&s[second_start..second_start + ms(0.13)]);
        assert!((first - 196.0).abs() < 6.0, "first pair at {first} Hz");
        assert!((second - 185.0).abs() < 6.0, "second pair at {second} Hz");
    }

    #[test]
    fn presets_and_choices() {
        let mut a = AlertSounds::default();
        assert_eq!(a.preset(), Some(Preset::Alr67));
        assert_eq!(a.get(Alert::Error), Sound::RwrLaunch);
        *a.get_mut(Alert::Error) = Sound::A1Error;
        assert_eq!(a.preset(), None);
        assert_eq!(Preset::BambuA1.sounds().error, Sound::A1Error);
        // Every sound in a preset is in the library list.
        for p in Preset::ALL {
            for alert in Alert::ALL {
                assert!(Sound::ALL.contains(&p.sounds().get(alert)));
            }
        }
    }

    #[test]
    fn wav_header() {
        let w = wav(&[0, 1000, -1000]);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), RATE);
        assert_eq!(&w[36..40], b"data");
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 6);
        assert_eq!(w.len(), 44 + 6);
    }

    /// Plays a silent sound through the real Windows API and checks Windows
    /// accepts the generated WAV. Needs an audio device, so run it by hand:
    /// `cargo test plays_through_windows -- --ignored`
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn plays_through_windows() {
        #[link(name = "winmm")]
        unsafe extern "system" {
            fn PlaySoundW(sound: *const u16, module: *mut std::ffi::c_void, flags: u32) -> i32;
        }
        let bytes = wav(&synthesize(Sound::RwrStatus, 0.0));
        // SAFETY: SND_MEMORY (4) with a complete WAV image that outlives the call.
        let ok = unsafe { PlaySoundW(bytes.as_ptr().cast(), std::ptr::null_mut(), 0x0004 | 0x0002) };
        assert_ne!(ok, 0, "PlaySound rejected the WAV");
    }

    #[test]
    fn repeats_are_rate_limited() {
        let mut p = Player::default();
        p.last.insert(Alert::Error, Instant::now());
        let before = p.last[&Alert::Error];
        p.alert(Sound::Silent, Alert::Error, 0.0);
        assert_eq!(p.last[&Alert::Error], before);
    }
}
