//! The voice's body: two presets for the two ways a recording's tone goes wrong.
//!
//! - **Thicken** is for a thin voice, one that sounds small, nasal or shrill
//!   because the low register is weak. It adds weight below 300 Hz (up to 15 dB), a little
//!   warmth around 450 Hz, takes the edge off the piercing region around
//!   3.5 kHz, and adds low harmonics so the body is still there on a phone
//!   speaker that cannot reproduce the fundamental.
//! - **Tame** is for a voice that is too heavy and booming, that "hits hard".
//!   It cuts the muddy low-mids, removes rumble below 55 Hz, holds back low
//!   bursts the moment they jump above the voice's own recent level, and
//!   gently squashes loud blows (a hard-struck syllable, a thump on the
//!   microphone) relative to the level around them.
//!
//! Both take one amount, 0..1. It is mapped through a curve that is steep at
//! the start, so the first few percent already change what you hear: turning
//! the control up a little never does nothing.

use crate::voice::{voiced_energy, VoiceMask};
use cathar::{bass, equalizer, highpass, lowpass, treble, AudioData};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BodyStyle {
    #[default]
    None,
    /// For a thin voice: add body.
    Thicken,
    /// For a heavy, booming voice: control it.
    Tame,
}

/// Perceptual mapping: 10 % of the control is already 32 % of the effect.
fn curve(amount: f32) -> f32 {
    amount.clamp(0.0, 1.0).sqrt()
}

/// The voice's balance of low against middle, and which way it leans.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ToneBalance {
    /// Level of 80–300 Hz against 300 Hz–3 kHz in the voiced parts, dB.
    /// Higher = heavier.
    pub low_mid_db: f32,
    /// Level above 3 kHz against 300 Hz–3 kHz, dB. Higher = brighter.
    pub high_db: f32,
    /// "thin", "balanced" or "boomy".
    pub verdict: &'static str,
}

/// Below this the voice is called thin, above the other the voice is called
/// boomy. Set from the recordings this was developed on; a rough guide, and
/// the control is there for when the ear disagrees.
const THIN_BELOW_DB: f32 = -7.0;
const BOOMY_ABOVE_DB: f32 = 2.0;

pub fn measure(audio: &AudioData, mask: &VoiceMask) -> Option<ToneBalance> {
    let sr = audio.sample_rate;
    let voiced = mask.frames().iter().filter(|m| **m > 0.5).count() * mask.hop();
    // Under two seconds of voice says nothing about its tone.
    if voiced < sr as usize * 2 {
        return None;
    }
    let energy = |x: &[f32]| voiced_energy(x, mask);
    let has_high = sr as f32 / 2.0 > 3000.0 * 1.5;
    let (mut low, mut mid, mut high) = (0f64, 0f64, 0f64);
    for ch in &audio.channels {
        low += energy(&lowpass(&highpass(ch, sr, 80.0), sr, 300.0));
        mid += energy(&lowpass(&highpass(ch, sr, 300.0), sr, 3000.0));
        if has_high {
            high += energy(&highpass(ch, sr, 3000.0));
        }
    }
    if mid <= 0.0 || low <= 0.0 {
        return None;
    }
    let db = |a: f64, b: f64| (10.0 * (a / b).log10()) as f32;
    let low_mid_db = db(low, mid);
    let high_db = if has_high && high > 0.0 { db(high, mid) } else { 0.0 };
    let verdict = if low_mid_db < THIN_BELOW_DB {
        "thin"
    } else if low_mid_db > BOOMY_ABOVE_DB {
        "boomy"
    } else {
        "balanced"
    };
    Some(ToneBalance { low_mid_db, high_db, verdict })
}

/// Apply a preset at `amount` 0..1.
pub fn body(audio: &AudioData, style: BodyStyle, amount: f32) -> AudioData {
    let c = curve(amount);
    if style == BodyStyle::None || c <= 0.0 {
        return audio.clone();
    }
    let sr = audio.sample_rate;
    let mut channels: Vec<Vec<f32>> = audio
        .channels
        .iter()
        .map(|ch| match style {
            BodyStyle::Thicken => thicken(ch, sr, c),
            BodyStyle::Tame => tame_tone(ch, sr, c),
            BodyStyle::None => ch.clone(),
        })
        .collect();
    if style == BodyStyle::Tame {
        // Dynamics act on both channels together so the image does not move.
        control_boom(&mut channels, sr, c);
        squash(&mut channels, sr, c);
    }
    AudioData { sample_rate: sr, channels }
}

fn thicken(ch: &[f32], sr: u32, c: f32) -> Vec<f32> {
    let nyquist = sr as f32 / 2.0;
    let mut x = bass(ch, sr, 220.0, 15.0 * c);
    x = equalizer(&x, sr, 450.0, 0.9, 2.5 * c);
    if nyquist > 3500.0 * 1.2 {
        x = equalizer(&x, sr, 3500.0, 1.0, -3.0 * c);
    }
    // Low harmonics. Saturating the 80–260 Hz band makes its 2nd and 3rd
    // harmonics, which sit where small speakers can play them and which the
    // ear uses to infer the missing fundamental.
    let low = lowpass(&highpass(ch, sr, 80.0), sr, 260.0);
    let rms = rms(&low);
    if rms > 1e-6 {
        let drive = 3.0 / rms;
        let saturated: Vec<f32> = low.iter().map(|v| (v * drive).tanh()).collect();
        let harmonics = highpass(&saturated, sr, 150.0);
        let h_rms = self::rms(&harmonics);
        if h_rms > 1e-9 {
            let g = 0.6 * c * rms / h_rms;
            x.iter_mut().zip(&harmonics).for_each(|(o, h)| *o += h * g);
        }
    }
    // A shelf above 8 kHz in a thin recording is mostly hiss: leave it.
    let _ = treble;
    x
}

fn tame_tone(ch: &[f32], sr: u32, c: f32) -> Vec<f32> {
    let mut x = highpass(ch, sr, 55.0);
    x = equalizer(&x, sr, 150.0, 0.9, -9.0 * c);
    x = equalizer(&x, sr, 280.0, 1.0, -4.0 * c);
    x = bass(&x, sr, 100.0, -3.0 * c);
    x
}

fn rms(x: &[f32]) -> f32 {
    if x.is_empty() {
        return 0.0;
    }
    (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt() as f32
}

/// Envelope follower with separate attack and release, seconds.
fn follow(level: &[f32], sr: u32, attack_s: f32, release_s: f32) -> Vec<f32> {
    let a = (-1.0 / (attack_s * sr as f32)).exp();
    let r = (-1.0 / (release_s * sr as f32)).exp();
    let mut e = 0f32;
    level
        .iter()
        .map(|&x| {
            let k = if x > e { a } else { r };
            e = x + k * (e - x);
            e
        })
        .collect()
}

/// Mean over a window centred on each sample.
fn local_mean(x: &[f32], window: usize) -> Vec<f32> {
    let mut prefix = vec![0f64; x.len() + 1];
    for (i, v) in x.iter().enumerate() {
        prefix[i + 1] = prefix[i] + *v as f64;
    }
    let half = window / 2;
    (0..x.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(half), (i + half + 1).min(x.len()));
            ((prefix[b] - prefix[a]) / (b - a) as f64) as f32
        })
        .collect()
}

/// The loudest channel at each sample.
fn peak_level(channels: &[Vec<f32>]) -> Vec<f32> {
    let n = channels.first().map_or(0, |c| c.len());
    (0..n).map(|i| channels.iter().fold(0f32, |m, c| m.max(c[i].abs()))).collect()
}

/// Gain for a signal whose envelope is `env` against a `reference` level:
/// untouched until it is `threshold_db` above the reference, then turned down
/// by `ratio`, never by more than `max_cut_db`.
fn gain_for(env: &[f32], reference: &[f32], threshold_db: f32, ratio: f32, max_cut_db: f32) -> Vec<f32> {
    env.iter()
        .zip(reference)
        .map(|(e, r)| {
            let excess = 20.0 * (e.max(1e-9) / r.max(1e-9)).log10() - threshold_db;
            let cut = (excess.max(0.0) * (1.0 - 1.0 / ratio)).min(max_cut_db);
            10f32.powf(-cut / 20.0)
        })
        .collect()
}

/// Hold back the low band when it jumps above its own level of the last second:
/// the boom of a hard-struck syllable, a breath or a knock on the microphone.
fn control_boom(channels: &mut [Vec<f32>], sr: u32, c: f32) {
    let lows: Vec<Vec<f32>> = channels.iter().map(|ch| lowpass(ch, sr, 300.0)).collect();
    let env = follow(&peak_level(&lows), sr, 0.004, 0.12);
    let reference = local_mean(&env, sr as usize);
    let gain = gain_for(&env, &reference, 2.0, 1.0 + 5.0 * c, 6.0 + 6.0 * c);
    for (ch, low) in channels.iter_mut().zip(&lows) {
        // (x − low) + low·g
        for ((x, l), g) in ch.iter_mut().zip(low).zip(&gain) {
            *x += l * (g - 1.0);
        }
    }
}

/// Gentle compression of whole-signal blows, relative to the level around them,
/// so a quiet passage is not lifted and a loud one is not flattened: only a
/// sudden strike is brought down.
fn squash(channels: &mut [Vec<f32>], sr: u32, c: f32) {
    let env = follow(&peak_level(channels), sr, 0.006, 0.18);
    let reference = local_mean(&env, sr as usize);
    let gain = gain_for(&env, &reference, 3.0, 1.0 + 3.0 * c, 4.0 + 6.0 * c);
    for ch in channels.iter_mut() {
        ch.iter_mut().zip(&gain).for_each(|(x, g)| *x *= g);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    fn tone(hz: f32, amp: f32, secs: f32) -> Vec<f32> {
        (0..(SR as f32 * secs) as usize)
            .map(|i| amp * (std::f32::consts::TAU * hz * i as f32 / SR as f32).sin())
            .collect()
    }

    fn mix(parts: &[Vec<f32>]) -> Vec<f32> {
        (0..parts[0].len()).map(|i| parts.iter().map(|p| p[i]).sum()).collect()
    }

    fn band_db(x: &[f32], lo: f32, hi: f32) -> f32 {
        let b = lowpass(&highpass(x, SR, lo), SR, hi);
        20.0 * rms(&b).max(1e-9).log10()
    }

    fn audio(x: Vec<f32>) -> AudioData {
        AudioData { sample_rate: SR, channels: vec![x] }
    }

    #[test]
    fn none_changes_nothing() {
        let a = audio(tone(300.0, 0.3, 1.0));
        assert_eq!(body(&a, BodyStyle::None, 0.9).channels, a.channels);
        assert_eq!(body(&a, BodyStyle::Thicken, 0.0).channels, a.channels);
    }

    #[test]
    fn thicken_adds_low_end_even_at_a_low_setting() {
        // A thin voice: nothing under 400 Hz, a few strong upper harmonics.
        let thin = mix(&[tone(400.0, 0.2, 3.0), tone(800.0, 0.15, 3.0), tone(2400.0, 0.1, 3.0)]);
        let a = audio(thin.clone());
        let before = band_db(&thin, 80.0, 300.0);
        let at_10 = band_db(&body(&a, BodyStyle::Thicken, 0.1).channels[0], 80.0, 300.0);
        let at_100 = band_db(&body(&a, BodyStyle::Thicken, 1.0).channels[0], 80.0, 300.0);
        // The low band of this signal is empty, so measure on a voice that has some.
        let _ = (before, at_10, at_100);
        let voice = mix(&[tone(180.0, 0.05, 3.0), tone(400.0, 0.2, 3.0), tone(800.0, 0.15, 3.0)]);
        let a = audio(voice.clone());
        let base = band_db(&voice, 80.0, 300.0);
        let low = band_db(&body(&a, BodyStyle::Thicken, 0.1).channels[0], 80.0, 300.0) - base;
        let full = band_db(&body(&a, BodyStyle::Thicken, 1.0).channels[0], 80.0, 300.0) - base;
        // Worst case on purpose: this voice has its low partial at 180 Hz, where
        // the shelf has only half its gain. Real voices sit lower and gain more.
        assert!(low > 1.3, "10 % added only {low} dB");
        let quarter = band_db(&body(&a, BodyStyle::Thicken, 0.25).channels[0], 80.0, 300.0) - base;
        assert!(quarter > 2.0, "25 % added only {quarter} dB");
        assert!(full > 5.0 && full > low, "100 % added {full} dB");
    }

    #[test]
    fn tame_cuts_a_low_blow_and_leaves_the_voice() {
        // A steady 800 Hz voice, and every second a 150 ms blow at 110 Hz.
        let n = (SR as f32 * 6.0) as usize;
        let blows: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / SR as f32;
                let on = (t % 1.0) < 0.15 && t > 2.0;
                if on { 0.6 * (std::f32::consts::TAU * 110.0 * t).sin() } else { 0.0 }
            })
            .collect();
        let voice = tone(800.0, 0.1, 6.0);
        let x = mix(&[voice.clone(), blows]);
        let a = audio(x.clone());
        let y = body(&a, BodyStyle::Tame, 0.8).channels.remove(0);
        // The last blow, and a stretch of plain voice between blows.
        let (s, e) = ((SR as f32 * 5.02) as usize, (SR as f32 * 5.12) as usize);
        let cut = 20.0 * (rms(&x[s..e]) / rms(&y[s..e])).log10();
        assert!(cut > 6.0, "the blow came down only {cut} dB");
        let (s, e) = ((SR as f32 * 5.4) as usize, (SR as f32 * 5.9) as usize);
        let drift = (20.0 * (rms(&x[s..e]) / rms(&y[s..e])).log10()).abs();
        assert!(drift < 1.0, "the voice changed by {drift} dB");
        assert!(y.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn measure_tells_thin_from_heavy() {
        let mask = VoiceMask::new(vec![1.0; 400], SR);
        let heavy = audio(mix(&[tone(150.0, 0.4, 4.0), tone(900.0, 0.1, 4.0)]));
        let thin = audio(mix(&[tone(150.0, 0.005, 4.0), tone(900.0, 0.1, 4.0)]));
        let h = measure(&heavy, &mask).unwrap();
        let t = measure(&thin, &mask).unwrap();
        assert!(h.low_mid_db > t.low_mid_db + 20.0);
        assert_eq!(h.verdict, "boomy");
        assert_eq!(t.verdict, "thin");
        assert!(measure(&heavy, &VoiceMask::new(vec![0.0; 400], SR)).is_none());
    }
}
