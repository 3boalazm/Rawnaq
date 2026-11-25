//! Noise detector and profile denoiser.
//!
//! Tape hiss, room tone and electrical noise are steady: their spectrum barely
//! changes over a recording, while the voice moves all the time. So the noise
//! is measured once, as a spectrum, from the level each frequency falls back
//! to when nothing else is there ([`learn_noise`]), and then only what sits at
//! that measured floor is turned down ([`subtract_noise`]). Anything clearly
//! above the floor, which is where the voice lives, passes unchanged. That is
//! what makes this engine safe to run *under* the voice, where the neural
//! denoiser is held back.
//!
//! The noise under the voice is measured separately from the noise in the
//! gaps. Many recordings have been gated or compressed before, or carry noise
//! that rides on the signal (tape modulation, a codec), so the floor between
//! the voice's harmonics is higher than the floor between phrases. A profile
//! taken from the gaps alone leaves that noise in, and the result "breathes":
//! silent gaps, noisy voice.

use cathar::AudioData;
use realfft::RealFftPlanner;
use serde::Serialize;

/// Frames quieter than this are digital silence and say nothing about noise.
const SILENCE_DB: f32 = -80.0;
/// The measured floor is scaled up by this before subtracting, so noise that
/// happens to peak above its average is still caught.
const OVER: f32 = 2.5;
/// Deepest cut at full strength: −24 dB. Never to zero, which sounds dead.
const FLOOR: f32 = 0.063;
/// Voice score from which a frame counts as voiced when measuring.
const VOICED: f32 = 0.6;

#[derive(Debug, Clone, Serialize)]
pub struct NoiseProfile {
    /// Transform size the profile was measured with.
    #[serde(skip)]
    pub fft: usize,
    /// Mean noise power per frequency bin between phrases.
    #[serde(skip)]
    pub power: Vec<f32>,
    /// Mean noise power per frequency bin while a voice is present.
    #[serde(skip)]
    pub voiced_power: Vec<f32>,
    /// Overall level of the steady noise, dBFS.
    pub level_dbfs: f32,
    /// Level of the noise under the voice, dBFS. Higher than `level_dbfs` when
    /// the noise rises with the voice.
    pub voiced_dbfs: f32,
    /// Level below 250 Hz (rumble, hum), dBFS.
    pub low_dbfs: f32,
    /// Level from 250 Hz to 3 kHz (room, crowd murmur), dBFS.
    pub mid_dbfs: f32,
    /// Level above 3 kHz (hiss), dBFS.
    pub high_dbfs: f32,
    /// The band that carries most of the noise: "low", "mid" or "high".
    pub kind: &'static str,
}

/// About 85 ms: fine enough in frequency to reach between the harmonics of a
/// low voice, which are as little as 80 Hz apart.
fn fft_size(sample_rate: u32) -> usize {
    if sample_rate >= 32_000 {
        4096
    } else {
        2048
    }
}

fn window(fft: usize) -> Vec<f32> {
    // Square-root Hann: applied before and after the transform, the two
    // together overlap-add to a constant at 75 % overlap.
    (0..fft)
        .map(|i| (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / fft as f32).cos()).sqrt())
        .collect()
}

/// Measure the steady noise, between phrases and under the voice.
///
/// Between phrases the noise is simply what the pauses hold. Under the voice,
/// each frequency is looked at on its own and the level it falls back to in
/// its quietest tenth of the time is taken as its noise: a voice moves from
/// note to note, so every frequency is free of it some of the time, even
/// while the reciter is in the middle of a phrase.
///
/// `score` is the voice score per 10 ms frame.
pub fn learn_noise(audio: &AudioData, score: &[f32]) -> NoiseProfile {
    /// Share of the time a frequency is assumed to hold nothing but noise.
    const QUIET_SHARE: f32 = 0.10;
    /// Frames examined at most, spread evenly over the recording.
    const MAX_FRAMES: usize = 3000;
    /// Fewer voiced frames than this are too few to measure from.
    const MIN_VOICED: usize = 100;
    /// Fewer pause frames than this (about 2 s) are too few to measure from.
    const MIN_GAPS: usize = 100;
    /// Voice score up to which a frame counts as a pause.
    const GAP: f32 = 0.15;

    let sr = audio.sample_rate as f32;
    let fft_n = fft_size(audio.sample_rate);
    let (hop, bins) = (fft_n / 4, fft_n / 2 + 1);
    let n = audio.channels.first().map_or(0, |c| c.len());
    let n_frames = if n >= fft_n { (n - fft_n) / hop + 1 } else { 0 };
    let n_ch = audio.channels.len().max(1) as f32;

    let audible: Vec<usize> = (0..n_frames)
        .filter(|f| {
            let s = f * hop;
            let e: f32 = audio
                .channels
                .iter()
                .map(|c| c[s..s + fft_n].iter().map(|v| v * v).sum::<f32>())
                .sum::<f32>()
                / (fft_n as f32 * n_ch);
            10.0 * e.max(1e-12).log10() > SILENCE_DB
        })
        .collect();
    let voiced_at = |f: usize| {
        let centre = (f * hop + fft_n / 2) as f32 / sr * 100.0;
        score.get(centre as usize).is_some_and(|v| *v >= VOICED)
    };
    let thin = |frames: Vec<usize>| {
        let stride = frames.len().div_ceil(MAX_FRAMES).max(1);
        frames.into_iter().step_by(stride).collect::<Vec<_>>()
    };
    let gap_at = |f: usize| {
        let centre = ((f * hop + fft_n / 2) as f32 / sr * 100.0) as usize;
        // Well clear of any voice: nothing within 100 ms either side.
        let (a, b) = (centre.saturating_sub(10), centre + 10);
        b < score.len() && score[a..=b].iter().all(|v| *v <= GAP)
    };
    let voiced = thin(audible.iter().copied().filter(|f| voiced_at(*f)).collect());
    let gaps = thin(audible.iter().copied().filter(|f| gap_at(*f)).collect());
    let all = thin(audible);

    let win = window(fft_n);
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(fft_n);
    let measure = |frames: &[usize], from_gaps: bool| {
        let mut power = vec![0f32; bins];
        if frames.is_empty() {
            return power;
        }
        let (mut buf, mut spec) = (fft.make_input_vec(), fft.make_output_vec());
        // Per bin: its power in every examined frame.
        let mut columns = vec![Vec::with_capacity(frames.len()); bins];
        for &f in frames {
            let mut frame = vec![0f32; bins];
            for c in &audio.channels {
                buf.iter_mut().zip(&c[f * hop..]).zip(&win).for_each(|((b, s), w)| *b = s * w);
                let _ = fft.process(&mut buf, &mut spec);
                frame.iter_mut().zip(&spec).for_each(|(p, x)| *p += x.norm_sqr() / n_ch);
            }
            columns.iter_mut().zip(&frame).for_each(|(col, p)| col.push(*p));
        }
        // Noise power in one bin is exponentially distributed: the value it
        // stays under a share q of the time is −ln(1 − q) times its mean.
        let to_mean = 1.0 / -(1.0 - QUIET_SHARE).ln();
        for (p, col) in power.iter_mut().zip(&mut columns) {
            col.sort_by(|a, b| a.total_cmp(b));
            *p = if from_gaps {
                // These frames hold noise only, so their average is the
                // noise. The loudest tenth is left out: a cough, a page
                // turning. For noise that leaves 0.67 of the true mean.
                let kept = &col[..(col.len() * 9 / 10).max(1)];
                kept.iter().sum::<f32>() / kept.len() as f32 / 0.67
            } else {
                col[(col.len() as f32 * QUIET_SHARE) as usize] * to_mean
            };
        }
        power
    };
    // With enough pauses, measure the noise in them directly. A low
    // percentile is only the fallback for a recording with no pause: it reads
    // too low on compressed sources (MP3, AAC), which leave holes in the
    // spectrum.
    let power = if gaps.len() >= MIN_GAPS { measure(&gaps, true) } else { measure(&all, false) };
    // Under the voice the floor is never lower than between phrases.
    let voiced_power: Vec<f32> = if voiced.len() >= MIN_VOICED {
        measure(&voiced, false).iter().zip(&power).map(|(v, p)| v.max(*p)).collect()
    } else {
        power.clone()
    };

    // Parseval with this window: variance = 4 · Σ power / fft².
    let band = |power: &[f32], lo: f32, hi: f32| {
        let sum: f32 = power
            .iter()
            .enumerate()
            .filter(|(k, _)| {
                let hz = *k as f32 * sr / fft_n as f32;
                hz >= lo && hz < hi
            })
            .map(|(_, p)| *p)
            .sum();
        (10.0 * (4.0 * sum / (fft_n * fft_n) as f32).max(1e-12).log10()).max(-120.0)
    };
    let (low, mid, high) =
        (band(&power, 0.0, 250.0), band(&power, 250.0, 3000.0), band(&power, 3000.0, sr));
    let kind = if low >= mid && low >= high {
        "low"
    } else if high >= mid {
        "high"
    } else {
        "mid"
    };
    NoiseProfile {
        fft: fft_n,
        level_dbfs: band(&power, 0.0, sr),
        voiced_dbfs: band(&voiced_power, 0.0, sr),
        low_dbfs: low,
        mid_dbfs: mid,
        high_dbfs: high,
        kind,
        power,
        voiced_power,
    }
}

/// Turn down, at full strength, whatever sits at the measured noise floor:
/// the floor measured under the voice where `score` (per 10 ms frame) says a
/// voice is, the floor measured between phrases elsewhere. The caller mixes
/// the result with the untouched signal for less strength.
pub fn subtract_noise(audio: &AudioData, profile: &NoiseProfile, score: &[f32]) -> AudioData {
    let sr = audio.sample_rate;
    let channels = std::thread::scope(|s| {
        let handles: Vec<_> = audio
            .channels
            .iter()
            .map(|c| s.spawn(move || subtract_channel(c, sr, profile, score)))
            .collect();
        handles
            .into_iter()
            .zip(&audio.channels)
            .map(|(h, c)| h.join().unwrap_or_else(|_| c.clone()))
            .collect()
    });
    AudioData { sample_rate: sr, channels }
}

fn subtract_channel(x: &[f32], sr: u32, profile: &NoiseProfile, score: &[f32]) -> Vec<f32> {
    let n = x.len();
    let fft_n = profile.fft;
    let (hop, bins) = (fft_n / 4, fft_n / 2 + 1);
    if n < fft_n || profile.voiced_power.iter().all(|p| *p <= 0.0) {
        return x.to_vec();
    }
    let win = window(fft_n);
    let mut planner = RealFftPlanner::<f32>::new();
    let (fwd, inv) = (planner.plan_fft_forward(fft_n), planner.plan_fft_inverse(fft_n));
    let (mut buf, mut spec) = (fwd.make_input_vec(), fwd.make_output_vec());
    // Pad so the first and last samples are covered by four frames like the rest.
    let pad = fft_n - hop;
    let total = (n + 2 * pad).div_ceil(hop) * hop + fft_n;
    let mut padded = vec![0f32; total];
    padded[pad..pad + n].copy_from_slice(x);
    let mut out = vec![0f32; total];

    let mut held = vec![0f32; bins]; // signal power, smoothed over time
    let mut power = vec![0f32; bins];
    let mut gain = vec![0f32; bins];
    // The window applied twice is a Hann window, which sums to 2 at 75 %
    // overlap; the inverse transform is unnormalised.
    let scale = 1.0 / (2.0 * fft_n as f32);

    for start in (0..=total - fft_n).step_by(hop) {
        buf.iter_mut().zip(&padded[start..]).zip(&win).for_each(|((b, s), w)| *b = s * w);
        let _ = fwd.process(&mut buf, &mut spec);
        for (p, x) in power.iter_mut().zip(&spec) {
            *p = x.norm_sqr();
        }
        // How much of a voice this frame holds decides which floor applies.
        let centre = (start + fft_n / 2).saturating_sub(pad) as f32 / sr as f32 * 100.0;
        let v = score.get(centre as usize).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        for k in 0..bins {
            // Average with the two neighbouring bins and over time: a lone
            // bin that pokes above the floor for one frame is noise, not
            // voice, and is what makes a denoiser "twinkle". No wider, or the
            // space between two harmonics would count as part of them.
            let (a, b) = (k.saturating_sub(1), (k + 1).min(bins - 1));
            let local = power[a..=b].iter().sum::<f32>() / (b - a + 1) as f32;
            // Quick to rise (a voice starting is let through at once), slower
            // to fall (its tail is not chopped).
            let rise = if local > held[k] { 0.6 } else { 0.45 };
            held[k] += rise * (local - held[k]);
            let noise = profile.power[k] + (profile.voiced_power[k] - profile.power[k]) * v;
            gain[k] = (1.0 - OVER * noise / held[k].max(1e-20)).max(0.0).sqrt().max(FLOOR);
        }
        for (x, g) in spec.iter_mut().zip(&gain) {
            *x *= *g;
        }
        let _ = inv.process(&mut spec, &mut buf);
        out[start..start + fft_n]
            .iter_mut()
            .zip(&buf)
            .zip(&win)
            .for_each(|((o, s), w)| *o += s * w * scale);
    }
    out[pad..pad + n].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(n: usize, amp: f32) -> Vec<f32> {
        let mut seed = 0x2545_f491u32;
        (0..n)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed as f32 / u32::MAX as f32 * 2.0 - 1.0) * amp
            })
            .collect()
    }

    fn rms_db(x: &[f32]) -> f32 {
        10.0 * (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).log10()
    }

    #[test]
    fn removes_hiss_and_keeps_the_tone() {
        let sr = 44_100usize;
        let hiss = noise(sr * 4, 0.01);
        // A loud tone in the second half only.
        let x: Vec<f32> = hiss
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let tone = if i >= sr * 2 { 0.3 * (i as f32 * 440.0 * std::f32::consts::TAU / sr as f32).sin() } else { 0.0 };
                h + tone
            })
            .collect();
        let audio = AudioData { sample_rate: sr as u32, channels: vec![x.clone()] };
        let profile = learn_noise(&audio, &[]);
        // Uniform noise of amplitude 0.01 has an RMS of −44.8 dBFS.
        assert!((profile.level_dbfs + 44.8).abs() < 1.5, "level {}", profile.level_dbfs);
        let y = subtract_noise(&audio, &profile, &[]).channels.remove(0);
        let quiet = sr / 2..sr * 3 / 2;
        let cut = rms_db(&x[quiet.clone()]) - rms_db(&y[quiet]);
        assert!(cut > 10.0, "hiss cut by only {cut} dB");
        let loud = sr * 5 / 2..sr * 7 / 2;
        let change = (rms_db(&x[loud.clone()]) - rms_db(&y[loud])).abs();
        assert!(change < 0.2, "tone changed by {change} dB");
    }

    #[test]
    fn silence_and_short_input_pass_through() {
        let audio = AudioData { sample_rate: 44_100, channels: vec![vec![0.0; 100]] };
        let p = learn_noise(&audio, &[]);
        assert_eq!(subtract_noise(&audio, &p, &[]).channels[0].len(), 100);
    }
}
