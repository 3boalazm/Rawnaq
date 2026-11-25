//! After the denoiser: give the voice back its tone and its level.
//!
//! Nothing here invents sound, with one exception the user has to ask for:
//! the optional reverberation ([`crate::reverb`]). The rest is shelving EQ and
//! gain, the same tools a mastering engineer would use, applied to what is
//! already in the recording.

use crate::reverb::{reverb, ReverbStyle};
use cathar::{bass, equalizer, highpass, integrated_loudness, lowpass, treble, AudioData};
use serde::{Deserialize, Serialize};

/// Below this the voice's body lives; the low shelf acts here.
const LOW_HZ: f32 = 250.0;
/// Above this consonant clarity lives; the high shelf acts here.
const HIGH_HZ: f32 = 3000.0;
/// Largest automatic boost. The high band is capped lower because part of what
/// the denoiser removed up there was hiss, not voice.
const MAX_LOW_RESTORE_DB: f32 = 6.0;
const MAX_HIGH_RESTORE_DB: f32 = 4.0;
/// Centre of each equaliser band, Hz. The first and last are shelves.
pub const EQ_HZ: [f32; 5] = [100.0, 300.0, 1000.0, 3000.0, 8000.0];
/// Output never exceeds this, whatever the gain.
const CEILING_DBFS: f32 = -1.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EnhanceSettings {
    /// Measure what the denoiser took from the lows and highs of the voice and
    /// put it back.
    pub tone_restore: bool,
    /// Extra low shelf, dB (−6..+6). Positive = fuller.
    pub warmth_db: f32,
    /// Extra high shelf, dB (−6..+6). Positive = clearer.
    pub clarity_db: f32,
    /// Bring the recording up to `target_lufs`.
    pub loudness: bool,
    pub target_lufs: f32,
    /// Five-band equaliser, dB per band (−12..+12), at [`EQ_HZ`].
    pub eq_db: [f32; 5],
    /// Master output gain, dB (−12..+24), applied after everything else and
    /// before the limiter.
    pub master_db: f32,
    /// Write a mono recording to both left and right channels.
    pub stereo: bool,
    /// The space added around the voice; `None` adds nothing.
    pub reverb: ReverbStyle,
    /// 0..1, how loud that space is next to the voice.
    pub reverb_amount: f32,
}

impl Default for EnhanceSettings {
    fn default() -> Self {
        Self {
            tone_restore: true,
            warmth_db: 0.0,
            clarity_db: 0.0,
            loudness: true,
            target_lufs: -18.0,
            eq_db: [0.0; 5],
            master_db: 0.0,
            stereo: false,
            reverb: ReverbStyle::None,
            reverb_amount: 0.4,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct EnhanceReport {
    /// How much more the voice's lows dropped than the voice overall, dB.
    pub low_loss_db: f32,
    /// Same for the highs.
    pub high_loss_db: f32,
    /// Shelf gains actually applied (automatic + manual), dB.
    pub low_gain_db: f32,
    pub high_gain_db: f32,
    /// Loudness gain applied, dB.
    pub gain_db: f32,
    /// Largest gain reduction the limiter needed, dB (0 = it never acted).
    pub limited_db: f32,
}

fn db(e_in: f64, e_out: f64) -> f32 {
    if e_in <= 0.0 {
        return 0.0;
    }
    (10.0 * (e_in / e_out.max(1e-20)).log10()).clamp(-60.0, 60.0) as f32
}

/// Energy of `x` where the voice mask is on.
fn voiced_energy(x: &[f32], mask: &[f32]) -> f64 {
    x.iter().zip(mask).filter(|(_, m)| **m > 0.5).map(|(s, _)| (*s as f64).powi(2)).sum()
}

/// How much more the lows and highs of the voice dropped than the voice as a
/// whole, comparing the denoiser's input and output inside the voice mask.
/// Returns `(low, high)` in dB; positive = that band lost more.
pub fn tone_loss(before: &AudioData, after: &AudioData, mask: &[f32]) -> (f32, f32) {
    let sr = before.sample_rate;
    let has_high = sr as f32 / 2.0 > HIGH_HZ * 1.5;
    let (mut all, mut low, mut high) = ((0f64, 0f64), (0f64, 0f64), (0f64, 0f64));
    for (b, a) in before.channels.iter().zip(&after.channels) {
        all.0 += voiced_energy(b, mask);
        all.1 += voiced_energy(a, mask);
        low.0 += voiced_energy(&lowpass(b, sr, LOW_HZ), mask);
        low.1 += voiced_energy(&lowpass(a, sr, LOW_HZ), mask);
        if has_high {
            high.0 += voiced_energy(&highpass(b, sr, HIGH_HZ), mask);
            high.1 += voiced_energy(&highpass(a, sr, HIGH_HZ), mask);
        }
    }
    let overall = db(all.0, all.1);
    (db(low.0, low.1) - overall, if has_high { db(high.0, high.1) - overall } else { 0.0 })
}

/// EQ, then the optional reverberation, then loudness, then a limiter.
/// `loss` comes from [`tone_loss`].
pub fn enhance(audio: &AudioData, s: &EnhanceSettings, loss: (f32, f32)) -> (AudioData, EnhanceReport) {
    let sr = audio.sample_rate;
    let mut report = EnhanceReport { low_loss_db: loss.0, high_loss_db: loss.1, ..Default::default() };

    let auto = |v: f32, max: f32| if s.tone_restore { v.clamp(0.0, max) } else { 0.0 };
    report.low_gain_db = auto(loss.0, MAX_LOW_RESTORE_DB) + s.warmth_db.clamp(-6.0, 6.0);
    report.high_gain_db = auto(loss.1, MAX_HIGH_RESTORE_DB) + s.clarity_db.clamp(-6.0, 6.0);
    let has_high = sr as f32 / 2.0 > HIGH_HZ * 1.5;
    if !has_high {
        report.high_gain_db = 0.0;
    }

    let mut channels: Vec<Vec<f32>> = audio
        .channels
        .iter()
        .map(|ch| {
            let mut x = ch.clone();
            if report.low_gain_db.abs() > 0.05 {
                x = bass(&x, sr, LOW_HZ, report.low_gain_db);
            }
            if report.high_gain_db.abs() > 0.05 {
                x = treble(&x, sr, HIGH_HZ, report.high_gain_db);
            }
            for (band, (hz, gain)) in EQ_HZ.iter().zip(s.eq_db).enumerate() {
                let gain = gain.clamp(-12.0, 12.0);
                // A band above what the recording can hold has nothing to act on.
                if gain.abs() < 0.05 || *hz >= sr as f32 * 0.45 {
                    continue;
                }
                x = match band {
                    0 => bass(&x, sr, *hz, gain),
                    4 => treble(&x, sr, *hz, gain),
                    _ => equalizer(&x, sr, *hz, 1.0, gain),
                };
            }
            x
        })
        .collect();

    // Before the reverberation, so a mono recording gets a different space on
    // each side instead of the same one twice.
    if s.stereo && channels.len() == 1 {
        channels.push(channels[0].clone());
    }
    if s.reverb != ReverbStyle::None {
        let wet = reverb(&AudioData { sample_rate: sr, channels }, s.reverb, s.reverb_amount);
        channels = wet.channels;
    }

    if s.loudness {
        let measured = integrated_loudness(&channels, sr);
        if measured.is_finite() {
            report.gain_db = (s.target_lufs.clamp(-30.0, -8.0) - measured).clamp(-20.0, 36.0);
            let g = 10f32.powf(report.gain_db / 20.0);
            channels.iter_mut().flatten().for_each(|v| *v *= g);
        }
    }
    let master = s.master_db.clamp(-12.0, 24.0);
    if master.abs() > 0.01 {
        let g = 10f32.powf(master / 20.0);
        channels.iter_mut().flatten().for_each(|v| *v *= g);
    }
    report.limited_db = limit(&mut channels, sr, CEILING_DBFS);

    (AudioData { sample_rate: sr, channels }, report)
}

/// Look-ahead peak limiter, channels linked. Gain starts falling 5 ms before a
/// peak and recovers over ~80 ms, so peaks are turned down rather than clipped.
/// Returns the largest reduction in dB.
fn limit(channels: &mut [Vec<f32>], sr: u32, ceiling_dbfs: f32) -> f32 {
    let n = channels.first().map_or(0, |c| c.len());
    let ceiling = 10f32.powf(ceiling_dbfs / 20.0);
    let needed: Vec<f32> = (0..n)
        .map(|i| {
            let peak = channels.iter().fold(0f32, |m, c| m.max(c[i].abs()));
            if peak > ceiling {
                ceiling / peak
            } else {
                1.0
            }
        })
        .collect();
    let deepest = needed.iter().fold(1f32, |m, g| m.min(*g));
    if deepest >= 1.0 {
        return 0.0;
    }

    let look = (sr as usize / 200).max(1); // 5 ms
    // Minimum of `needed` over [i, i + look], by a monotonic queue.
    let mut ahead = vec![1f32; n];
    let mut q: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    let mut j = 0;
    for (i, a) in ahead.iter_mut().enumerate() {
        while j < n && j <= i + look {
            while q.back().is_some_and(|b| needed[*b] >= needed[j]) {
                q.pop_back();
            }
            q.push_back(j);
            j += 1;
        }
        while q.front().is_some_and(|f| *f < i) {
            q.pop_front();
        }
        *a = needed[*q.front().unwrap()];
    }
    // Slow recovery (never above what is needed).
    let release = 1.0 - (-1.0 / (0.08 * sr as f32)).exp();
    let mut held = 1f32;
    for a in ahead.iter_mut() {
        held = a.min(held + (1.0 - held) * release);
        *a = held;
    }
    // Average over the look-ahead: every window averaged at sample i contains
    // i, so the result is still at or below `needed[i]`, but without steps.
    let mut sum = 0f64;
    let mut gain = vec![1f32; n];
    for i in 0..n {
        sum += ahead[i] as f64;
        if i > look {
            sum -= ahead[i - look - 1] as f64;
        }
        gain[i] = (sum / (i.min(look) + 1) as f64) as f32;
    }
    for ch in channels.iter_mut() {
        for (s, g) in ch.iter_mut().zip(&gain) {
            *s *= g;
        }
    }
    -20.0 * deepest.log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_holds_the_ceiling_without_touching_quiet_audio() {
        let sr = 48_000;
        let mut x: Vec<f32> =
            (0..sr).map(|i| (i as f32 * 0.05).sin() * if (20_000..20_400).contains(&i) { 2.0 } else { 0.3 }).collect();
        let quiet_before = x[1000];
        let mut chans = vec![std::mem::take(&mut x)];
        let reduced = limit(&mut chans, sr as u32, -1.0);
        let peak = chans[0].iter().fold(0f32, |m, s| m.max(s.abs()));
        assert!(peak <= 10f32.powf(-1.0 / 20.0) + 1e-4, "peak {peak}");
        assert!(reduced > 6.0);
        assert!((chans[0][1000] - quiet_before).abs() < 1e-5);
    }

    #[test]
    fn loudness_reaches_the_target() {
        let sr = 48_000u32;
        let ch: Vec<f32> = (0..sr * 5).map(|i| (i as f32 * 0.06).sin() * 0.02).collect();
        let a = AudioData { sample_rate: sr, channels: vec![ch] };
        let s = EnhanceSettings { tone_restore: false, ..Default::default() };
        let (out, rep) = enhance(&a, &s, (0.0, 0.0));
        let l = integrated_loudness(&out.channels, sr);
        assert!((l - s.target_lufs).abs() < 0.5, "loudness {l}, gain {}", rep.gain_db);
    }
}
